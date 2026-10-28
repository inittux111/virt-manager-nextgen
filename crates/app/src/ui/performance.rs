//! The Performance tab: larger graphs of one machine over the last
//! minutes, with read/write and in/out shown separately.

use adw::prelude::*;
use vmng_virt::{DomainState, DomainSummary};

use super::widgets::{Chart, card, reflow};
use crate::backend::{HISTORY_LEN, VmLive};
use crate::format;

struct ChartCard {
    root: gtk::Box,
    value: gtk::Label,
    legend: gtk::Box,
    chart: Chart,
    legend_values: Vec<gtk::Label>,
}

fn chart_card(title: &str, metric: &str, legend: &[(&str, bool)]) -> ChartCard {
    let label = gtk::Label::builder().label(title).xalign(0.0).hexpand(true).css_classes(["heading"]).build();
    let value = gtk::Label::builder().xalign(1.0).css_classes(["title-3", "numeric"]).build();
    let head = gtk::Box::builder().spacing(8).build();
    head.append(&label);
    head.append(&value);
    let chart = Chart::new(metric, HISTORY_LEN, "");
    let legend_box = gtk::Box::builder().spacing(16).build();
    let mut legend_values = Vec::new();
    for (name, dashed) in legend {
        let swatch = gtk::Box::builder().valign(gtk::Align::Center).css_classes(["legend-swatch", metric]).build();
        if *dashed {
            swatch.add_css_class("dashed");
        }
        let text = gtk::Label::builder().label(*name).css_classes(["caption", "dim-label"]).build();
        let v = gtk::Label::builder().css_classes(["caption", "numeric"]).build();
        let item = gtk::Box::builder().spacing(6).build();
        item.append(&swatch);
        item.append(&text);
        item.append(&v);
        legend_box.append(&item);
        legend_values.push(v);
    }
    let root = card(gtk::Orientation::Vertical, 10);
    root.add_css_class("chart-card");
    root.set_hexpand(true);
    root.append(&head);
    root.append(&chart);
    root.append(&legend_box);
    root.update_property(&[gtk::accessible::Property::Label(title)]);
    ChartCard { root, value, legend: legend_box, chart, legend_values }
}

pub struct PerformanceView {
    pub root: gtk::Stack,
    cpu: ChartCard,
    memory: ChartCard,
    disk: ChartCard,
    net: ChartCard,
}

impl std::fmt::Debug for PerformanceView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PerformanceView").finish_non_exhaustive()
    }
}

impl PerformanceView {
    pub fn new() -> Self {
        let cpu = chart_card("CPU", "cpu", &[]);
        cpu.chart.set_max(Some(100.0));
        cpu.chart.set_format(|v| format!("{v:.0}%"));
        cpu.legend.set_visible(false);
        let memory = chart_card("Memory", "memory", &[]);
        memory.chart.set_format(|v| format::bytes(v as u64));
        memory.legend.set_visible(false);
        let disk = chart_card("Disk I/O", "disk", &[("Read", false), ("Write", true)]);
        disk.chart.set_format(format::rate);
        disk.chart.set_floor(100_000.0);
        let net = chart_card("Network", "network", &[("In", false), ("Out", true)]);
        net.chart.set_format(format::rate);
        net.chart.set_floor(10_000.0);

        // Two by two, or one column in narrow windows.
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            .css_classes(["page-content"])
            .build();
        let mut rows = Vec::new();
        for pair in [[&cpu.root, &memory.root], [&disk.root, &net.root]] {
            let row = gtk::Box::builder().spacing(16).homogeneous(true).build();
            row.append(pair[0]);
            row.append(pair[1]);
            body.append(&row);
            rows.push(row);
        }
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&adw::Clamp::builder().maximum_size(1180).child(&body).build())
            .build();
        let charts = reflow(&scroller, &[&rows[0], &rows[1]], &[], 700.0);
        let off = adw::StatusPage::builder()
            .icon_name("utilities-system-monitor-symbolic")
            .title("The machine isn't running")
            .description("Start it to see how busy it is.")
            .build();
        let root = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
        root.add_named(&charts, Some("charts"));
        root.add_named(&off, Some("off"));
        Self { root, cpu, memory, disk, net }
    }

    /// Updates the charts; `interval` is the statistics interval in seconds.
    pub fn update(&self, vm: &DomainSummary, live: Option<&VmLive>, interval: u32) {
        let active = vm.state.is_active();
        self.root.set_visible_child_name(if active { "charts" } else { "off" });
        let Some(l) = live.filter(|_| active) else { return };
        let minutes = (HISTORY_LEN as u32 * interval.max(1)).div_ceil(60);
        let span = format!("{minutes} min ago");
        for c in [&self.cpu, &self.memory, &self.disk, &self.net] {
            c.chart.set_span(&span);
        }
        let running = vm.state == DomainState::Running;

        self.cpu.value.set_label(&l.cpu_pct.filter(|_| running).map_or_else(|| "—".into(), format::percent));
        self.cpu.chart.set_series(vec![l.cpu.values().collect()]);

        self.memory.value.set_label(&l.mem_used.map_or_else(|| "—".into(), |u| format::used_of(u, vm.config.memory)));
        self.memory.chart.set_max(Some(vm.config.memory.max(1) as f64));
        self.memory.chart.set_series(vec![l.mem.values().collect()]);

        self.disk.value.set_label(&format::rate(l.disk_read + l.disk_write));
        self.disk.legend_values[0].set_label(&format::rate(l.disk_read));
        self.disk.legend_values[1].set_label(&format::rate(l.disk_write));
        self.disk.chart.set_series(vec![l.disk_rd.values().collect(), l.disk_wr.values().collect()]);

        self.net.value.set_label(&format::rate(l.net_rx + l.net_tx));
        self.net.legend_values[0].set_label(&format::rate(l.net_rx));
        self.net.legend_values[1].set_label(&format::rate(l.net_tx));
        self.net.chart.set_series(vec![l.net_in.values().collect(), l.net_out.values().collect()]);
    }
}
