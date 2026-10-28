//! Layouts that adapt to narrow windows.
//!
//! Each one wraps its content in an `adw::BreakpointBin`, so it reacts to
//! the room it actually gets rather than to the window as a whole.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

/// Smallest size every adaptive layout can shrink to (a phone-sized window).
const MIN_WIDTH: i32 = 360;
const MIN_HEIGHT: i32 = 200;

/// Pixels rather than `sp`: the panes themselves have fixed pixel widths.
fn breakpoint(max_width: f64) -> adw::Breakpoint {
    adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        max_width,
        adw::LengthUnit::Px,
    ))
}

fn bin(child: &impl IsA<gtk::Widget>) -> adw::BreakpointBin {
    adw::BreakpointBin::builder().width_request(MIN_WIDTH).height_request(MIN_HEIGHT).child(child).build()
}

/// Boxes laid out side by side that stack into one column (no longer
/// equal-sized) when `child` is narrower than `max_width`; `flows` keep at
/// least two columns until then, and may drop to one below it.
pub fn reflow(
    child: &impl IsA<gtk::Widget>,
    boxes: &[&gtk::Box],
    flows: &[&gtk::FlowBox],
    max_width: f64,
) -> adw::BreakpointBin {
    let root = bin(child);
    let bp = breakpoint(max_width);
    for b in boxes {
        bp.add_setter(*b, "orientation", Some(&gtk::Orientation::Vertical.to_value()));
        bp.add_setter(*b, "homogeneous", Some(&false.to_value()));
    }
    for f in flows {
        f.set_min_children_per_line(2);
        bp.add_setter(*f, "min-children-per-line", Some(&1u32.to_value()));
    }
    root.add_breakpoint(bp);
    root
}

/// A padded, scrolled page frame (`page-content`, clamped to `max_width`).
pub fn scrolled_page(content: &gtk::Box, max_width: i32) -> gtk::Widget {
    content.add_css_class("page-content");
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&adw::Clamp::builder().maximum_size(max_width).child(content).build())
        .build()
        .upcast()
}

type LayoutFn = Box<dyn Fn(bool)>;

/// A list beside its details. When narrower than the threshold only one
/// of the two shows at a time, and the details get a back button (Escape
/// or Alt+Left also go back).
#[derive(Clone)]
pub struct ListDetail(Rc<Inner>);

struct Inner {
    root: adw::BreakpointBin,
    stack: gtk::Stack,
    wide: gtk::Box,
    /// Where the list and the details go in the narrow layout.
    list_holder: gtk::Box,
    detail_holder: gtk::Box,
    list: gtk::Widget,
    /// The list's own `hexpand`/`vexpand`, restored in the wide layout.
    list_expand: (bool, bool),
    detail_column: gtk::Box,
    narrow: Cell<bool>,
    showing_detail: Cell<bool>,
    on_layout: RefCell<Vec<LayoutFn>>,
}

impl std::fmt::Debug for ListDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ListDetail").field("narrow", &self.0.narrow.get()).finish_non_exhaustive()
    }
}

impl ListDetail {
    /// `list` and `detail` side by side `spacing` pixels apart (with a
    /// separator line if `separator`), with `back_label` naming the list on
    /// the narrow layout's back button. `frame` wraps each page's box, e.g.
    /// in a padded scrolled view, so that every page scrolls on its own.
    pub fn new(
        list: &impl IsA<gtk::Widget>,
        detail: &impl IsA<gtk::Widget>,
        spacing: i32,
        separator: bool,
        back_label: &str,
        max_width: f64,
        frame: impl Fn(&gtk::Box) -> gtk::Widget,
    ) -> Self {
        let back = gtk::Button::builder()
            .child(&adw::ButtonContent::builder().icon_name("go-previous-symbolic").label(back_label).build())
            .css_classes(["flat"])
            .tooltip_text(format!("Back to {}", back_label.to_lowercase()))
            .build();
        let back_bar = gtk::Box::builder().spacing(8).css_classes(["back-bar"]).build();
        back_bar.append(&back);
        let detail_column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).build();
        detail_column.append(&back_bar);
        detail_column.append(detail);

        let wide = gtk::Box::builder().spacing(spacing).build();
        wide.append(list);
        if separator {
            wide.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        }
        wide.append(&detail_column);

        // Narrow: the list and the details become two pages that slide.
        // Not homogeneous: the hidden side-by-side page mustn't hold the
        // window at its own minimum width.
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::SlideLeftRight)
            .hhomogeneous(false)
            .vhomogeneous(false)
            .build();
        let list_holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let detail_holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
        stack.add_named(&frame(&wide), Some("wide"));
        stack.add_named(&frame(&list_holder), Some("list"));
        stack.add_named(&frame(&detail_holder), Some("detail"));
        let root = bin(&stack);

        let this = Self(Rc::new(Inner {
            root,
            stack,
            wide,
            list_holder,
            detail_holder,
            list: list.clone().upcast(),
            list_expand: (list.hexpands(), list.vexpands()),
            detail_column,
            narrow: Cell::new(false),
            showing_detail: Cell::new(false),
            on_layout: RefCell::default(),
        }));
        back_bar.set_visible(false);
        // Debug builds: `VMNG_DEV_DETAIL=1` starts narrow layouts on the details.
        #[cfg(debug_assertions)]
        this.0.showing_detail.set(std::env::var_os("VMNG_DEV_DETAIL").is_some());

        let bp = breakpoint(max_width);
        bp.add_setter(&back_bar, "visible", Some(&true.to_value()));
        let weak = Rc::downgrade(&this.0);
        bp.connect_apply(move |_| {
            if let Some(i) = weak.upgrade() {
                ListDetail(i).set_narrow(true);
            }
        });
        let weak = Rc::downgrade(&this.0);
        bp.connect_unapply(move |_| {
            if let Some(i) = weak.upgrade() {
                ListDetail(i).set_narrow(false);
            }
        });
        this.0.root.add_breakpoint(bp);

        let weak = Rc::downgrade(&this.0);
        back.connect_clicked(move |_| {
            if let Some(i) = weak.upgrade() {
                ListDetail(i).show_list();
            }
        });
        let keys = gtk::ShortcutController::new();
        let weak = Rc::downgrade(&this.0);
        keys.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("Escape|<Alt>Left"),
            Some(gtk::CallbackAction::new(move |_, _| {
                let Some(i) = weak.upgrade() else { return glib::Propagation::Proceed };
                if !(i.narrow.get() && i.showing_detail.get()) {
                    return glib::Propagation::Proceed;
                }
                ListDetail(i).show_list();
                glib::Propagation::Stop
            })),
        ));
        this.0.detail_column.add_controller(keys);
        this
    }

    pub fn root(&self) -> &adw::BreakpointBin {
        &self.0.root
    }

    /// Calls `f` with `true` when the layout turns narrow, `false` when wide.
    pub fn connect_layout(&self, f: impl Fn(bool) + 'static) {
        self.0.on_layout.borrow_mut().push(Box::new(f));
    }

    /// Whether only one pane shows at a time.
    pub fn is_narrow(&self) -> bool {
        self.0.narrow.get()
    }

    /// Brings the details into view (narrow layout only).
    pub fn show_detail(&self) {
        self.0.showing_detail.set(true);
        if self.is_narrow() {
            self.place_narrow();
        }
    }

    pub fn show_list(&self) {
        self.0.showing_detail.set(false);
        if self.is_narrow() {
            self.place_narrow();
        }
    }

    fn set_narrow(&self, narrow: bool) {
        let i = &self.0;
        if i.narrow.replace(narrow) == narrow {
            return;
        }
        for f in i.on_layout.borrow().iter() {
            f(narrow);
        }
        if narrow {
            // Stays on the details if the person was looking at an item.
            self.place_narrow();
        } else {
            // Back into the side-by-side box, without animation.
            i.stack.set_transition_type(gtk::StackTransitionType::None);
            i.list.unparent();
            i.detail_column.unparent();
            i.wide.prepend(&i.list);
            i.wide.append(&i.detail_column);
            i.list.set_hexpand(i.list_expand.0);
            i.list.set_vexpand(i.list_expand.1);
            i.stack.set_visible_child_name("wide");
            i.stack.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
        }
    }

    /// Puts the visible pane into its narrow page and slides to it.
    fn place_narrow(&self) {
        let i = &self.0;
        let (name, child, holder): (&str, &gtk::Widget, &gtk::Box) = if i.showing_detail.get() {
            ("detail", i.detail_column.upcast_ref(), &i.detail_holder)
        } else {
            ("list", &i.list, &i.list_holder)
        };
        if child.parent().as_ref() != Some(holder.upcast_ref()) {
            child.unparent();
            holder.append(child);
        }
        child.set_hexpand(true);
        child.set_vexpand(true);
        i.stack.set_visible_child_name(name);
        // A new page starts at the top, even inside a scrolled view.
        if let Some(scroller) =
            holder.ancestor(gtk::ScrolledWindow::static_type()).and_downcast::<gtk::ScrolledWindow>()
        {
            scroller.vadjustment().set_value(0.0);
        }
    }
}
