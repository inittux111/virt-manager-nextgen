//! A small line graph of recent values.
//!
//! The line colour is the widget's CSS `color`, so each metric gets its
//! colour from the stylesheet (`sparkline.cpu`, `.memory`, …) and the theme.

use std::cell::{Cell, RefCell};

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{graphene, gsk};

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct Sparkline {
        pub values: RefCell<Vec<f64>>,
        /// Fixed top of the scale; `None` scales to the largest value.
        pub max: Cell<Option<f64>>,
        /// Number of samples the full width represents.
        pub capacity: Cell<usize>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Sparkline {
        const NAME: &'static str = "VmngSparkline";
        type Type = super::Sparkline;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("sparkline");
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for Sparkline {}

    impl WidgetImpl for Sparkline {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => (40, 120, -1, -1),
                _ => (36, 36, -1, -1),
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let w = widget.width() as f32;
            let h = widget.height() as f32;
            let values = self.values.borrow();
            let color = widget.color();
            let stroke_width = 2.0_f32;
            let top = stroke_width;
            let bottom = h - stroke_width;
            let span = (bottom - top).max(1.0);

            // A faint baseline across the full width keeps the graph's extent
            // visible while history fills in, and reads as "no activity".
            let mut faint = color;
            faint.set_alpha(color.alpha() * 0.25);
            snapshot.append_color(&faint, &graphene::Rect::new(0.0, bottom, w, 1.0));
            if values.len() < 2 {
                return;
            }

            let cap = self.capacity.get().max(values.len()).max(2);
            let max =
                self.max.get().unwrap_or_else(|| values.iter().copied().fold(0.0_f64, f64::max)).max(f64::EPSILON);
            let step = w / (cap - 1) as f32;
            let x0 = w - step * (values.len() - 1) as f32;
            let point = |i: usize, v: f64| {
                let frac = (v / max).clamp(0.0, 1.0) as f32;
                graphene::Point::new(x0 + step * i as f32, bottom - frac * span)
            };

            let line = gsk::PathBuilder::new();
            let area = gsk::PathBuilder::new();
            area.move_to(x0, h);
            for (i, &v) in values.iter().enumerate() {
                let p = point(i, v);
                if i == 0 {
                    line.move_to(p.x(), p.y());
                } else {
                    line.line_to(p.x(), p.y());
                }
                area.line_to(p.x(), p.y());
            }
            area.line_to(w, h);
            area.close();

            let mut fill = color;
            fill.set_alpha(color.alpha() * 0.14);
            snapshot.append_fill(&area.to_path(), gsk::FillRule::Winding, &fill);

            let stroke = gsk::Stroke::new(stroke_width);
            stroke.set_line_join(gsk::LineJoin::Round);
            stroke.set_line_cap(gsk::LineCap::Round);
            snapshot.append_stroke(&line.to_path(), &stroke, &color);
        }
    }
}

glib::wrapper! {
    pub struct Sparkline(ObjectSubclass<imp::Sparkline>) @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sparkline {
    /// Creates a graph for a metric (`cpu`, `memory`, `disk` or `network`).
    pub fn new(metric: &str, capacity: usize) -> Self {
        let s: Self = glib::Object::new();
        s.add_css_class(metric);
        s.imp().capacity.set(capacity);
        s.set_hexpand(true);
        s
    }

    pub fn set_max(&self, max: Option<f64>) {
        self.imp().max.set(max);
        self.queue_draw();
    }

    pub fn set_values(&self, values: impl IntoIterator<Item = f64>) {
        let mut v = self.imp().values.borrow_mut();
        v.clear();
        v.extend(values);
        drop(v);
        self.queue_draw();
    }
}
