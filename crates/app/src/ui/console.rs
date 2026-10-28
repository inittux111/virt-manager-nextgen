//! The console page: the guest's screen (SPICE or VNC) or its text console,
//! with a floating toolbar for keys, screenshots, scaling and fullscreen.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};
use vmng_display::{
    Backend, FdProvider, GuestScreen, Scaling, SpiceBackend, Status, Terminal, VncBackend, keymap, parse_keys,
};
use vmng_virt::{ConsoleStream, DomainSummary};

use super::feedback::Feedback;
use super::vm_actions::VmButtons;
use super::vm_list::{logo_key, os_label, state_text};
use super::widgets::{OsLogo, StatusLabel, Tone};
use crate::backend::Conn;
use crate::settings::{ConsoleScaling, Settings};

/// Key combinations offered under "Send Keys", as evdev codes.
fn key_combos() -> Vec<(&'static str, Vec<u32>)> {
    use keymap::keys::*;
    let mut v = vec![
        ("Ctrl+Alt+Del", vec![LEFT_CTRL, LEFT_ALT, DELETE]),
        ("Ctrl+Alt+Backspace", vec![LEFT_CTRL, LEFT_ALT, BACKSPACE]),
    ];
    for (n, label) in [
        (1, "Ctrl+Alt+F1"),
        (2, "Ctrl+Alt+F2"),
        (3, "Ctrl+Alt+F3"),
        (4, "Ctrl+Alt+F4"),
        (5, "Ctrl+Alt+F5"),
        (6, "Ctrl+Alt+F6"),
        (7, "Ctrl+Alt+F7"),
    ] {
        v.push((label, vec![LEFT_CTRL, LEFT_ALT, f(n)]));
    }
    v.push(("Print Screen", vec![PRINT]));
    v.push(("Super (Windows key)", vec![LEFT_SUPER]));
    v
}

/// The text console's connection: a reader thread fills `incoming`, a
/// writer thread sends what the person types.
struct SerialSession {
    stream: ConsoleStream,
    writer: mpsc::Sender<Vec<u8>>,
    poll: Option<glib::SourceId>,
}

impl Drop for SerialSession {
    fn drop(&mut self) {
        self.stream.close();
        if let Some(id) = self.poll.take() {
            id.remove();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Screen,
    Text,
}

pub struct ConsolePage {
    pub page: adw::NavigationPage,
    toolbar_view: adw::ToolbarView,
    stack: gtk::Stack,
    screen: GuestScreen,
    terminal: Terminal,
    logo: OsLogo,
    name: gtk::Label,
    status: StatusLabel,
    buttons: VmButtons,
    caption: gtk::Label,
    error_page: adw::StatusPage,
    mode_screen: gtk::ToggleButton,
    mode_text: gtk::ToggleButton,
    fullscreen: gtk::ToggleButton,
    resize_guest: gtk::ToggleButton,
    osd_screen_tools: gtk::Box,
    osd: gtk::Box,
    osd_revealer: gtk::Revealer,
    caption_revealer: gtk::Revealer,
    osd_menus: Vec<gtk::MenuButton>,
    osd_hovered: Cell<bool>,
    hide_timer: RefCell<Option<glib::SourceId>>,
    overlay: gtk::Overlay,
    scale_label: adw::ButtonContent,
    state: RefCell<PageState>,
    resize_timer: RefCell<Option<glib::SourceId>>,
    settings: Settings,
    feedback: Feedback,
}

#[derive(Default)]
struct PageState {
    conn: Option<Conn>,
    vm: Option<DomainSummary>,
    mode: Option<Mode>,
    serial: Option<SerialSession>,
    protocol: String,
    connected: bool,
    /// Bumped on each (re)connect, so stale callbacks are ignored.
    generation: u32,
}

impl std::fmt::Debug for ConsolePage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsolePage").finish_non_exhaustive()
    }
}

fn osd_button(icon: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::builder().icon_name(icon).tooltip_text(tooltip).css_classes(["flat", "circular"]).build();
    b.update_property(&[gtk::accessible::Property::Label(tooltip)]);
    b
}

fn osd_toggle(icon: &str, tooltip: &str) -> gtk::ToggleButton {
    let b =
        gtk::ToggleButton::builder().icon_name(icon).tooltip_text(tooltip).css_classes(["flat", "circular"]).build();
    b.update_property(&[gtk::accessible::Property::Label(tooltip)]);
    b
}

impl ConsolePage {
    pub const TAG: &'static str = "console";

    pub fn new(settings: Settings, feedback: Feedback) -> Rc<Self> {
        // Header: who and how it's doing, plus the usual controls.
        let logo = OsLogo::new(30);
        let name = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["heading"])
            .build();
        let status = StatusLabel::new();
        status.root.add_css_class("caption");
        status.root.add_css_class("dim-label");
        let titles = gtk::Box::builder().orientation(gtk::Orientation::Vertical).valign(gtk::Align::Center).build();
        titles.append(&name);
        titles.append(&status.root);
        let identity = gtk::Box::builder().spacing(10).build();
        identity.append(&logo);
        identity.append(&titles);
        let details =
            gtk::Button::builder().label("Details").action_name("vm.hardware").valign(gtk::Align::Center).build();
        let buttons = VmButtons::new().without_console();
        let header = adw::HeaderBar::builder().title_widget(&gtk::Box::new(gtk::Orientation::Horizontal, 0)).build();
        header.pack_start(&identity);
        header.pack_end(&buttons.root);
        header.pack_end(&details);

        // The screen and the text console.
        let screen = GuestScreen::new();
        screen.update_property(&[gtk::accessible::Property::Label("Guest display")]);
        let screen_scroller = gtk::ScrolledWindow::builder().child(&screen).css_classes(["console-surface"]).build();
        let terminal = Terminal::new();
        let term_frame = gtk::Box::builder().css_classes(["console-terminal"]).build();
        term_frame.append(terminal.widget());

        // Floating toolbar.
        let send_menu = gio::Menu::new();
        let combos = gio::Menu::new();
        for (i, (label, _)) in key_combos().iter().enumerate() {
            combos.append(Some(label), Some(&format!("console.send-keys({i})")));
        }
        send_menu.append_section(None, &combos);
        let send = gtk::MenuButton::builder()
            .menu_model(&send_menu)
            .child(&adw::ButtonContent::builder().icon_name("input-keyboard-symbolic").label("Send Keys").build())
            .css_classes(["flat"])
            .tooltip_text("Send a key combination to the guest")
            .build();
        let shot = osd_button("camera-photo-symbolic", "Take a screenshot");
        shot.set_action_name(Some("console.screenshot"));
        let scale_menu = gio::Menu::new();
        scale_menu.append(Some("Always scale to the window"), Some("console.scaling::always"));
        scale_menu.append(Some("Never scale (1:1 pixels)"), Some("console.scaling::never"));
        scale_menu.append(Some("Scale only in fullscreen"), Some("console.scaling::fullscreen"));
        let fit_window = gio::Menu::new();
        fit_window.append(Some("Resize Window to the Guest"), Some("console.fit-window"));
        scale_menu.append_section(None, &fit_window);
        let scale_label = adw::ButtonContent::builder().icon_name("zoom-fit-best-symbolic").label("Scale").build();
        let scale = gtk::MenuButton::builder()
            .child(&scale_label)
            .menu_model(&scale_menu)
            .tooltip_text("How the screen is scaled")
            .css_classes(["flat"])
            .build();
        let resize_guest = osd_toggle("video-display-symbolic", "Resize the guest to the window");
        let osd_screen_tools = gtk::Box::builder().spacing(2).build();
        osd_screen_tools.append(&send);
        let tools_sep = gtk::Separator::new(gtk::Orientation::Vertical);
        osd_screen_tools.append(&tools_sep);
        osd_screen_tools.append(&shot);
        osd_screen_tools.append(&scale);
        osd_screen_tools.append(&resize_guest);

        let mode_screen = gtk::ToggleButton::builder().label("Screen").css_classes(["flat"]).build();
        let mode_text =
            gtk::ToggleButton::builder().label("Text Console").css_classes(["flat"]).group(&mode_screen).build();
        let modes = gtk::Box::builder().spacing(2).build();
        modes.append(&mode_screen);
        modes.append(&mode_text);
        let fullscreen = osd_toggle("view-fullscreen-symbolic", "Fullscreen");

        let osd = gtk::Box::builder()
            .spacing(4)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Start)
            .margin_top(14)
            .css_classes(["osd", "console-toolbar"])
            .build();
        let seps = [
            tools_sep,
            gtk::Separator::new(gtk::Orientation::Vertical),
            gtk::Separator::new(gtk::Orientation::Vertical),
        ];
        osd.append(&modes);
        osd.append(&seps[1]);
        osd.append(&osd_screen_tools);
        osd.append(&seps[2]);
        osd.append(&fullscreen);
        osd.update_property(&[gtk::accessible::Property::Label("Console controls")]);

        let caption = gtk::Label::builder()
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .halign(gtk::Align::Start)
            .valign(gtk::Align::End)
            .margin_start(16)
            .margin_bottom(12)
            .css_classes(["caption", "osd", "console-caption"])
            .build();

        let error_page =
            adw::StatusPage::builder().icon_name("network-offline-symbolic").title("Not connected").build();
        let reconnect = gtk::Button::builder()
            .label("Reconnect")
            .action_name("console.reconnect")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        error_page.set_child(Some(&reconnect));
        let start = gtk::Button::builder()
            .label("Start")
            .action_name("vm.start")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        let off_page = adw::StatusPage::builder()
            .icon_name("system-shutdown-symbolic")
            .title("This machine is shut off")
            .description("Start it to see its screen.")
            .child(&start)
            .build();
        let connecting = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(14)
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .build();
        connecting.append(&adw::Spinner::builder().width_request(32).height_request(32).build());
        connecting
            .append(&gtk::Label::builder().label("Connecting to the display…").css_classes(["dim-label"]).build());

        let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
        stack.add_named(&connecting, Some("connecting"));
        stack.add_named(&screen_scroller, Some("screen"));
        stack.add_named(&term_frame, Some("text"));
        stack.add_named(&error_page, Some("error"));
        stack.add_named(&off_page, Some("off"));

        // In fullscreen the toolbar and caption hide themselves and come
        // back when the pointer touches the top edge (like virt-manager).
        let osd_revealer = gtk::Revealer::builder()
            .child(&osd)
            .reveal_child(true)
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Start)
            .build();
        let caption_revealer = gtk::Revealer::builder()
            .child(&caption)
            .reveal_child(true)
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .halign(gtk::Align::Start)
            .valign(gtk::Align::End)
            .build();
        let overlay = gtk::Overlay::builder().child(&stack).css_classes(["console-area"]).build();
        overlay.add_overlay(&osd_revealer);
        overlay.add_overlay(&caption_revealer);

        let toolbar_view = adw::ToolbarView::builder().content(&overlay).build();
        toolbar_view.add_top_bar(&header);
        // Narrow windows: shorter labels and fewer header extras.
        let bin = adw::BreakpointBin::builder().width_request(360).height_request(300).child(&toolbar_view).build();
        let compact = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            640.0,
            adw::LengthUnit::Px,
        ));
        compact.add_setter(&logo, "visible", Some(&false.to_value()));
        compact.add_setter(&details, "visible", Some(&false.to_value()));
        if let Some(content) = send.child().and_downcast::<adw::ButtonContent>() {
            compact.add_setter(&content, "label", Some(&"".to_value()));
        }
        compact.add_setter(&scale_label, "label", Some(&"".to_value()));
        compact.add_setter(&mode_text, "label", Some(&"Text".to_value()));
        for sep in &seps {
            compact.add_setter(sep, "visible", Some(&false.to_value()));
        }
        let b = buttons.clone();
        compact.connect_apply(move |_| b.set_compact(true));
        let b = buttons.clone();
        compact.connect_unapply(move |_| b.set_compact(false));
        bin.add_breakpoint(compact);
        let page = adw::NavigationPage::builder().tag(Self::TAG).title("Console").child(&bin).build();
        // F11, unless the guest has the keyboard (then the guest gets it).
        let keys = gtk::ShortcutController::new();
        let f = fullscreen.clone();
        keys.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("F11"),
            Some(gtk::CallbackAction::new(move |_, _| {
                f.set_active(!f.is_active());
                glib::Propagation::Stop
            })),
        ));
        page.add_controller(keys);

        let this = Rc::new(Self {
            page,
            toolbar_view,
            stack,
            screen,
            terminal,
            logo,
            name,
            status,
            buttons,
            caption,
            error_page,
            mode_screen,
            mode_text,
            fullscreen,
            resize_guest,
            osd_screen_tools,
            osd,
            osd_revealer,
            caption_revealer,
            osd_menus: vec![send, scale],
            osd_hovered: Cell::new(false),
            hide_timer: RefCell::default(),
            overlay,
            scale_label,
            state: RefCell::default(),
            resize_timer: RefCell::default(),
            settings,
            feedback,
        });
        this.setup_actions();
        this.wire();
        this.setup_autohide();
        this
    }

    fn setup_actions(self: &Rc<Self>) {
        let group = gio::SimpleActionGroup::new();
        let send = gio::SimpleAction::new("send-keys", Some(glib::VariantTy::INT32));
        let weak = Rc::downgrade(self);
        send.connect_activate(move |_, param| {
            let (Some(this), Some(i)) = (weak.upgrade(), param.and_then(|p| p.get::<i32>())) else { return };
            if let Some((_, keys)) = key_combos().get(i as usize) {
                this.screen.send_keys(keys);
            }
        });
        group.add_action(&send);

        let shot = gio::SimpleAction::new("screenshot", None);
        let weak = Rc::downgrade(self);
        shot.connect_activate(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.screenshot();
            }
        });
        group.add_action(&shot);

        let current = match self.settings.get().console_scaling {
            ConsoleScaling::Always => "always",
            ConsoleScaling::Never => "never",
            ConsoleScaling::FullscreenOnly => "fullscreen",
        };
        let scaling = gio::SimpleAction::new_stateful("scaling", Some(glib::VariantTy::STRING), &current.to_variant());
        let weak = Rc::downgrade(self);
        scaling.connect_activate(move |a, param| {
            let (Some(this), Some(v)) = (weak.upgrade(), param.and_then(|p| p.get::<String>())) else { return };
            a.set_state(&v.to_variant());
            let mode = match v.as_str() {
                "never" => ConsoleScaling::Never,
                "fullscreen" => ConsoleScaling::FullscreenOnly,
                _ => ConsoleScaling::Always,
            };
            this.settings.update(|p| p.console_scaling = mode);
            this.apply_scaling();
        });
        group.add_action(&scaling);

        let fit = gio::SimpleAction::new("fit-window", None);
        let weak = Rc::downgrade(self);
        fit.connect_activate(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.fit_window();
            }
        });
        group.add_action(&fit);

        let reconnect = gio::SimpleAction::new("reconnect", None);
        let weak = Rc::downgrade(self);
        reconnect.connect_activate(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.connect();
            }
        });
        group.add_action(&reconnect);
        self.page.insert_action_group("console", Some(&group));
    }

    fn wire(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.screen.connect_grab_changed(move |grabbed| {
            super::shortcuts::set_app_enabled(!grabbed);
            if let Some(this) = weak.upgrade() {
                this.update_caption();
            }
        });
        // The text console gets every key too (like a terminal app would).
        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter(|_| super::shortcuts::set_app_enabled(false));
        focus.connect_leave(|_| super::shortcuts::set_app_enabled(true));
        self.terminal.widget().add_controller(focus);
        let weak = Rc::downgrade(self);
        self.screen.connect_size_changed(move |w, h| {
            if let Some(this) = weak.upgrade() {
                this.schedule_guest_resize(w, h);
                this.update_caption();
            }
        });
        self.resize_guest.set_active(self.settings.get().console_resize_guest);
        let weak = Rc::downgrade(self);
        self.resize_guest.connect_toggled(move |b| {
            if let Some(this) = weak.upgrade() {
                let on = b.is_active();
                this.settings.update(|p| p.console_resize_guest = on);
                if on {
                    this.schedule_guest_resize(this.screen.width(), this.screen.height());
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.fullscreen.connect_toggled(move |b| {
            if let Some(this) = weak.upgrade() {
                this.set_fullscreen(b.is_active());
            }
        });
        let weak = Rc::downgrade(self);
        self.mode_text.connect_toggled(move |b| {
            if let Some(this) = weak.upgrade() {
                let mode = if b.is_active() { Mode::Text } else { Mode::Screen };
                if this.state.borrow().mode != Some(mode) {
                    this.state.borrow_mut().mode = Some(mode);
                    this.connect();
                }
            }
        });
        let terminal_weak = Rc::downgrade(self);
        self.terminal.connect_input(move |bytes| {
            if let Some(this) = terminal_weak.upgrade()
                && let Some(serial) = this.state.borrow().serial.as_ref()
            {
                let _ = serial.writer.send(bytes.to_vec());
            }
        });
    }

    /// Shows the console of `vm`. Connects if needed.
    pub fn open(self: &Rc<Self>, conn: Conn, vm: &DomainSummary) {
        let same = self.state.borrow().vm.as_ref().is_some_and(|v| v.uuid == vm.uuid);
        {
            let mut st = self.state.borrow_mut();
            st.conn = Some(conn);
            st.vm = Some(vm.clone());
            if !same {
                // Machines without a screen go straight to the text console.
                st.mode = Some(if vm.config.graphics.is_empty() { Mode::Text } else { Mode::Screen });
                // Debug builds: VMNG_DEV_CONSOLE_TEXT=1 opens the text console.
                #[cfg(debug_assertions)]
                if std::env::var_os("VMNG_DEV_CONSOLE_TEXT").is_some() {
                    st.mode = Some(Mode::Text);
                }
            }
        }
        self.update_header(vm);
        if !same || !self.state.borrow().connected {
            self.connect();
        }
    }

    /// Follows state changes while the page is open.
    pub fn update(self: &Rc<Self>, vm: &DomainSummary) {
        let was_active = self.state.borrow().vm.as_ref().is_some_and(|v| v.state.is_active());
        self.state.borrow_mut().vm = Some(vm.clone());
        self.update_header(vm);
        if !vm.state.is_active() {
            if was_active {
                self.disconnect();
            }
            self.stack.set_visible_child_name("off");
            self.update_caption();
        } else if !was_active {
            // It was started from here: connect now.
            self.connect();
        }
    }

    fn update_header(&self, vm: &DomainSummary) {
        self.page.set_title(&format!("{} – Console", vm.display_name()));
        self.logo.set_logo(logo_key(vm));
        self.name.set_label(vm.display_name());
        self.status.set(Tone::of(vm.state), &format!("{} · {}", state_text(vm), os_label(vm)));
        self.buttons.update(vm.state);
        let has_screen = !vm.config.graphics.is_empty();
        self.mode_screen.set_sensitive(has_screen);
    }

    /// Closes connections (when leaving the page or the VM stops).
    pub fn disconnect(&self) {
        let mut st = self.state.borrow_mut();
        st.generation += 1;
        st.connected = false;
        st.serial = None;
        drop(st);
        self.screen.set_backend(None);
        self.screen.ungrab();
    }

    pub fn leave(&self) {
        self.disconnect();
        if self.fullscreen.is_active() {
            self.fullscreen.set_active(false);
        }
        self.state.borrow_mut().vm = None;
    }

    fn connect(self: &Rc<Self>) {
        self.disconnect();
        let (conn, vm, mode, generation) = {
            let st = self.state.borrow();
            match (&st.conn, &st.vm) {
                (Some(c), Some(v)) => (c.clone(), v.clone(), st.mode.unwrap_or(Mode::Screen), st.generation),
                _ => return,
            }
        };
        if !vm.state.is_active() {
            self.stack.set_visible_child_name("off");
            return;
        }
        self.mode_screen.set_active(mode == Mode::Screen);
        self.mode_text.set_active(mode == Mode::Text);
        self.osd_screen_tools.set_visible(mode == Mode::Screen);
        self.stack.set_visible_child_name("connecting");
        match mode {
            Mode::Screen => self.connect_screen(conn, vm, generation),
            Mode::Text => self.connect_text(conn, vm, generation),
        }
    }

    fn current(&self, generation: u32) -> bool {
        self.state.borrow().generation == generation
    }

    fn connect_screen(self: &Rc<Self>, conn: Conn, vm: DomainSummary, generation: u32) {
        let protocol = vm.config.graphics.first().map(|g| g.kind.clone()).unwrap_or_default();
        self.state.borrow_mut().protocol = protocol.clone();
        self.apply_scaling();
        self.screen.set_release_keys(parse_keys(&self.settings.get().console_grab_keys));
        let uuid = vm.uuid.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let id = uuid.clone();
            let fd = conn.call(move |c| c.open_graphics_fd(&id, 0)).await;
            let Some(this) = weak.upgrade() else { return };
            if !this.current(generation) {
                return;
            }
            let fd = match fd {
                Ok(fd) => fd,
                Err(e) => return this.show_error("Couldn't connect to the display", e.message()),
            };
            let status = {
                let weak = Rc::downgrade(&this);
                move |s: Status| {
                    if let Some(this) = weak.upgrade()
                        && this.current(generation)
                    {
                        this.on_status(s);
                    }
                }
            };
            let backend: Rc<dyn Backend> = match protocol.as_str() {
                "spice" => {
                    let provider: FdProvider = {
                        let (conn, uuid) = (conn.clone(), uuid.clone());
                        Rc::new(move |reply| {
                            let (conn, uuid) = (conn.clone(), uuid.clone());
                            glib::spawn_future_local(async move {
                                let r = conn.call(move |c| c.open_graphics_fd(&uuid, 0)).await;
                                reply(r.map_err(|e| e.message().to_owned()));
                            });
                        })
                    };
                    SpiceBackend::connect(&this.screen, fd, provider, status)
                }
                "vnc" => VncBackend::connect(&this.screen, fd, status),
                other => {
                    return this.show_error(
                        "This display type isn't supported",
                        &format!("The machine uses “{other}”. Use SPICE or VNC to see its screen here."),
                    );
                }
            };
            this.screen.set_backend(Some(backend));
        });
    }

    fn on_status(self: &Rc<Self>, status: Status) {
        match status {
            Status::Connecting => self.stack.set_visible_child_name("connecting"),
            Status::Connected => {
                self.state.borrow_mut().connected = true;
                self.stack.set_visible_child_name("screen");
                self.screen.grab_focus();
                self.update_caption();
                // Size the guest to the window right away if wanted.
                let (w, h) = (self.screen.width(), self.screen.height());
                self.schedule_guest_resize(w, h);
                #[cfg(debug_assertions)]
                self.dev_keys();
            }
            Status::Disconnected(error) => {
                self.state.borrow_mut().connected = false;
                self.screen.ungrab();
                let running = self.state.borrow().vm.as_ref().is_some_and(|v| v.state.is_active());
                if !running {
                    self.stack.set_visible_child_name("off");
                } else {
                    self.show_error(
                        "The display connection was closed",
                        error.as_deref().unwrap_or("The machine may have rebooted or switched its display off."),
                    );
                }
            }
        }
    }

    fn show_error(&self, title: &str, details: &str) {
        self.error_page.set_title(title);
        self.error_page.set_description(Some(&glib::markup_escape_text(details)));
        self.stack.set_visible_child_name("error");
        self.caption.set_visible(false);
    }

    fn connect_text(self: &Rc<Self>, conn: Conn, vm: DomainSummary, generation: u32) {
        let uuid = vm.uuid.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            // Take over the console if someone else (virsh) holds it.
            let stream = conn.call(move |c| c.open_console(&uuid, true)).await;
            let Some(this) = weak.upgrade() else { return };
            if !this.current(generation) {
                return;
            }
            let stream = match stream {
                Ok(s) => s,
                Err(e) => return this.show_error("Couldn't open the text console", e.message()),
            };
            let incoming: Arc<Mutex<Vec<u8>>> = Arc::default();
            let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            {
                let (stream, incoming, closed) = (stream.clone(), incoming.clone(), closed.clone());
                let _ = std::thread::Builder::new().name("console-read".into()).spawn(move || {
                    let mut buf = [0u8; 4096];
                    while let Ok(n) = stream.read(&mut buf) {
                        if n == 0 {
                            break;
                        }
                        incoming.lock().unwrap_or_else(|p| p.into_inner()).extend_from_slice(&buf[..n]);
                    }
                    closed.store(true, std::sync::atomic::Ordering::Relaxed);
                });
            }
            let (writer, rx) = mpsc::channel::<Vec<u8>>();
            {
                let stream = stream.clone();
                let _ = std::thread::Builder::new().name("console-write".into()).spawn(move || {
                    for data in rx {
                        if stream.write(&data).is_err() {
                            break;
                        }
                    }
                });
            }
            // Move output to the terminal on the main thread, ~60 times a second.
            let weak = Rc::downgrade(&this);
            let poll = glib::timeout_add_local(Duration::from_millis(16), move || {
                let Some(this) = weak.upgrade() else { return glib::ControlFlow::Break };
                let data = std::mem::take(&mut *incoming.lock().unwrap_or_else(|p| p.into_inner()));
                this.terminal.feed(&data);
                if closed.load(std::sync::atomic::Ordering::Relaxed) {
                    if this.current(generation) {
                        this.state.borrow_mut().serial = None;
                        let running = this.state.borrow().vm.as_ref().is_some_and(|v| v.state.is_active());
                        if running {
                            this.show_error("The text console was closed", "Another program may have taken it over.");
                        }
                    }
                    return glib::ControlFlow::Break;
                }
                glib::ControlFlow::Continue
            });
            this.state.borrow_mut().serial = Some(SerialSession { stream, writer, poll: Some(poll) });
            this.state.borrow_mut().connected = true;
            this.stack.set_visible_child_name("text");
            this.terminal.widget().grab_focus();
            // A carriage return makes most guests print their login prompt.
            if let Some(s) = this.state.borrow().serial.as_ref() {
                let _ = s.writer.send(b"\r".to_vec());
            }
            this.update_caption();
        });
    }

    fn update_caption(&self) {
        let st = self.state.borrow();
        let text = match st.mode {
            Some(Mode::Text) if st.connected => "Text console · serial port".to_owned(),
            Some(Mode::Screen) if st.connected => {
                let size = self.screen.guest_size().map(|(w, h)| format!(" · {w}×{h}")).unwrap_or_default();
                let keys = self
                    .settings
                    .get()
                    .console_grab_keys
                    .replace("_L", "")
                    .replace("_R", "")
                    .replace("Control", "Ctrl");
                let hint = if self.screen.is_grabbed() {
                    format!("Press {keys} to release the keyboard")
                } else {
                    "Click the screen to type in it".to_owned()
                };
                format!("{}{size} · {hint}", st.protocol.to_uppercase())
            }
            _ => String::new(),
        };
        self.caption.set_label(&text);
        self.caption.set_visible(!text.is_empty());
    }

    fn apply_scaling(&self) {
        let pref = self.settings.get().console_scaling;
        let fit = match pref {
            ConsoleScaling::Always => true,
            ConsoleScaling::Never => false,
            ConsoleScaling::FullscreenOnly => self.fullscreen.is_active(),
        };
        self.screen.set_scaling(if fit { Scaling::Fit } else { Scaling::Native });
        self.scale_label.set_label(match pref {
            ConsoleScaling::Always => "Scale: always",
            ConsoleScaling::Never => "Scale: never",
            ConsoleScaling::FullscreenOnly => "Scale: fullscreen only",
        });
    }

    /// Asks the guest to match the window, half a second after the last
    /// size change (so dragging the window doesn't flood the guest).
    fn schedule_guest_resize(self: &Rc<Self>, width: i32, height: i32) {
        if !self.resize_guest.is_active() || width <= 0 || height <= 0 {
            return;
        }
        if let Some(id) = self.resize_timer.borrow_mut().take() {
            id.remove();
        }
        let weak = Rc::downgrade(self);
        let scale = self.screen.scale_factor().max(1);
        let id = glib::timeout_add_local_once(Duration::from_millis(500), move || {
            let Some(this) = weak.upgrade() else { return };
            this.resize_timer.replace(None);
            if let Some(b) = this.screen.backend()
                && b.can_resize_guest()
            {
                b.resize_guest((width * scale) as u32, (height * scale) as u32);
            }
        });
        self.resize_timer.replace(Some(id));
    }

    /// Makes the window just big enough to show the guest 1:1.
    fn fit_window(&self) {
        let Some((gw, gh)) = self.screen.guest_size() else { return };
        let Some(window) = self.page.root().and_downcast::<gtk::Window>() else { return };
        if window.is_maximized() || window.is_fullscreen() {
            window.unmaximize();
        }
        let (extra_w, extra_h) = (window.width() - self.screen.width(), window.height() - self.screen.height());
        window.set_default_size(gw + extra_w.max(0), gh + extra_h.max(0));
    }

    fn setup_autohide(self: &Rc<Self>) {
        // Watch the pointer over the whole console without taking events
        // away from the guest screen.
        let motion = gtk::EventControllerMotion::new();
        motion.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        motion.connect_motion(move |_, _, y| {
            if let Some(this) = weak.upgrade()
                && this.fullscreen.is_active()
                && y <= 8.0
            {
                this.show_osd();
            }
        });
        self.overlay.add_controller(motion);

        let hover = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        hover.connect_enter(move |_, _, _| {
            if let Some(this) = weak.upgrade() {
                this.osd_hovered.set(true);
                this.cancel_hide();
            }
        });
        let weak = Rc::downgrade(self);
        hover.connect_leave(move |_| {
            if let Some(this) = weak.upgrade() {
                this.osd_hovered.set(false);
                this.schedule_hide();
            }
        });
        self.osd.add_controller(hover);
        // Closing a menu of the toolbar counts as leaving it.
        for menu in &self.osd_menus {
            let weak = Rc::downgrade(self);
            menu.connect_active_notify(move |m| {
                if let Some(this) = weak.upgrade()
                    && !m.is_active()
                {
                    this.schedule_hide();
                }
            });
        }
    }

    fn show_osd(self: &Rc<Self>) {
        self.osd_revealer.set_reveal_child(true);
        self.caption_revealer.set_reveal_child(true);
        self.schedule_hide();
    }

    fn cancel_hide(&self) {
        if let Some(id) = self.hide_timer.borrow_mut().take() {
            id.remove();
        }
    }

    /// Hides the toolbar two seconds from now, if still in fullscreen and
    /// the person isn't using it.
    fn schedule_hide(self: &Rc<Self>) {
        self.cancel_hide();
        if !self.fullscreen.is_active() {
            return;
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(Duration::from_secs(2), move || {
            let Some(this) = weak.upgrade() else { return };
            this.hide_timer.replace(None);
            let menu_open = this.osd_menus.iter().any(gtk::MenuButton::is_active);
            if this.fullscreen.is_active() && !this.osd_hovered.get() && !menu_open {
                this.osd_revealer.set_reveal_child(false);
                this.caption_revealer.set_reveal_child(false);
            }
        });
        self.hide_timer.replace(Some(id));
    }

    fn set_fullscreen(self: &Rc<Self>, on: bool) {
        let Some(window) = self.page.root().and_downcast::<gtk::Window>() else { return };
        if on {
            window.fullscreen();
        } else {
            window.unfullscreen();
        }
        self.toolbar_view.set_reveal_top_bars(!on);
        self.fullscreen.set_icon_name(if on { "view-restore-symbolic" } else { "view-fullscreen-symbolic" });
        self.fullscreen.set_tooltip_text(Some(if on { "Leave fullscreen" } else { "Fullscreen" }));
        self.apply_scaling();
        if on {
            self.show_osd();
        } else {
            self.cancel_hide();
            self.osd_revealer.set_reveal_child(true);
            self.caption_revealer.set_reveal_child(true);
        }
    }

    /// Saves the current guest image as a PNG file.
    fn screenshot(&self) {
        let Some(texture) = self.screen.texture() else {
            self.feedback.toast("There's no picture to save yet.");
            return;
        };
        let name = self.state.borrow().vm.as_ref().map_or_else(|| "machine".to_owned(), |v| v.config.name.clone());
        let stamp = glib::DateTime::now_local()
            .and_then(|d| d.format("%Y-%m-%d %H.%M.%S"))
            .map(|s| s.to_string())
            .unwrap_or_default();
        let dialog = gtk::FileDialog::builder()
            .title("Save Screenshot")
            .initial_name(format!("{name} {stamp}.png"))
            .modal(true)
            .build();
        if let Some(pictures) = glib::user_special_dir(glib::UserDirectory::Pictures) {
            dialog.set_initial_folder(Some(&gio::File::for_path(pictures)));
        }
        let window = self.page.root().and_downcast::<gtk::Window>();
        let feedback = self.feedback.clone();
        glib::spawn_future_local(async move {
            let Ok(file) = dialog.save_future(window.as_ref()).await else { return };
            let Some(path) = file.path() else { return };
            match texture.save_to_png(&path) {
                Ok(()) => feedback.toast("Saved the screenshot"),
                Err(e) => feedback.error("Couldn't save the screenshot", &e.to_string()),
            }
        });
    }

    /// Debug builds: `VMNG_DEV_CONSOLE_KEYS=<evdev,evdev,…>` types those
    /// keys a second apart after connecting, to test keyboard input.
    #[cfg(debug_assertions)]
    fn dev_keys(self: &Rc<Self>) {
        // VMNG_DEV_CONSOLE_FULLSCREEN=1: switch to fullscreen after connecting.
        if std::env::var_os("VMNG_DEV_CONSOLE_FULLSCREEN").is_some() {
            self.fullscreen.set_active(true);
        }
        let Ok(list) = std::env::var("VMNG_DEV_CONSOLE_KEYS") else { return };
        let codes: Vec<u32> = list.split(',').filter_map(|c| c.trim().parse().ok()).collect();
        for (i, code) in codes.into_iter().enumerate() {
            let screen = self.screen.clone();
            glib::timeout_add_local_once(Duration::from_millis(1500 + 700 * i as u64), move || {
                screen.send_keys(&[code])
            });
        }
    }

    pub fn is_open_for(&self, uuid: &str) -> bool {
        self.state.borrow().vm.as_ref().is_some_and(|v| v.uuid == uuid)
    }
}
