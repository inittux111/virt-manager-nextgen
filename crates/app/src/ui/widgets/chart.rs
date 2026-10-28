//! A larger line chart with a scale: up to two series (the second drawn
//! dashed), light grid lines, the top value and the time span.
//!
//! Like `Sparkline`, the colour is the widget's CSS `color`, so charts use
//! the fixed metric colours (`chart.cpu`, `.memory`, `.disk`, `.network`).

use std::cell::{Cell, RefCell};

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{graphene, gsk};

/// Rounds up to 1, 2 or 5 times a power of ten (for a tidy scale).
pub fn nice_max(v: f64) -> f64 {
    if v <= 0.0 || !v.is_finite() {
        return 1.0;
    }
    let exp = v.log10().floor();
    let base = 10f64.powf(exp);
    let m = v / base;
    let nice = if m <= 1.0 {
        1.0
    } else if m <= 2.0 {
        2.0
    } else if m <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * base
}

type Format = Box<dyn Fn(f64) -> String>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Chart {
        pub series: RefCell<Vec<Vec<f64>>>,
        pub capacity: Cell<usize>,
        /// Fixed top of the scale (e.g. 100 % for CPU); `None` = automatic.
        pub max: Cell<Option<f64>>,
        /// Lowest top of an automatic scale, so idle noise stays flat.
        pub floor: Cell<f64>,
        pub format: RefCell<Option<Format>>,
        pub span: RefCell<String>,
    }

    impl std::fmt::Debug for Chart {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Chart").finish_non_exhaustive()
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Chart {
        const NAME: &'static str = "VmngChart";
        type Type = super::Chart;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("chart");
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for Chart {}

    impl WidgetImpl for Chart {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => (120, 400, -1, -1),
                _ => (140, 160, -1, -1),
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (w, h) = (widget.width() as f32, widget.height() as f32);
            let color = widget.color();
            let mut dim = color;
            dim.set_alpha(0.12);
            let label_h = 18.0_f32;
            let (top, bottom) = (label_h + 4.0, h - label_h - 4.0);
            let span = (bottom - top).max(1.0);

            // Grid: four horizontal lines.
            // Labels use the surrounding text colour, dimmed.
            let mut fg = widget.parent().map_or(color, |p| p.color());
            fg.set_alpha(fg.alpha() * 0.7);
            for i in 0..=3 {
                let y = top + span * i as f32 / 3.0;
                snapshot.append_color(&dim, &graphene::Rect::new(0.0, y.round(), w, 1.0));
            }

            let series = self.series.borrow();
            let auto = series.iter().flatten().copied().fold(0.0_f64, f64::max);
            let max = self.max.get().unwrap_or_else(|| nice_max(auto.max(self.floor.get()))).max(f64::EPSILON);
            let cap = self.capacity.get().max(2);
            let step = w / (cap - 1) as f32;

            for (n, values) in series.iter().enumerate() {
                if values.len() < 2 {
                    continue;
                }
                let x0 = w - step * (values.len() - 1) as f32;
                let path = gsk::PathBuilder::new();
                let area = gsk::PathBuilder::new();
                area.move_to(x0, bottom);
                for (i, &v) in values.iter().enumerate() {
                    let x = x0 + step * i as f32;
                    let y = bottom - (v / max).clamp(0.0, 1.0) as f32 * span;
                    if i == 0 {
                        path.move_to(x, y);
                    } else {
                        path.line_to(x, y);
                    }
                    area.line_to(x, y);
                }
                area.line_to(w, bottom);
                area.close();
                let stroke = gsk::Stroke::new(2.0);
                stroke.set_line_join(gsk::LineJoin::Round);
                if n == 0 {
                    let mut fill = color;
                    fill.set_alpha(color.alpha() * 0.14);
                    snapshot.append_fill(&area.to_path(), gsk::FillRule::Winding, &fill);
                } else {
                    stroke.set_dash(&[6.0, 4.0]);
                }
                snapshot.append_stroke(&path.to_path(), &stroke, &color);
            }

            // Labels: the scale's top value and the time span.
            let format = self.format.borrow();
            let top_label = format.as_ref().map_or_else(|| format!("{max:.0}"), |f| f(max));
            for (text, x_right, y) in [
                (top_label, false, 0.0),
                (self.span.borrow().clone(), false, h - label_h),
                ("now".to_owned(), true, h - label_h),
            ] {
                let layout = widget.create_pango_layout(Some(&text));
                let (lw, _) = layout.pixel_size();
                let x = if x_right { w - lw as f32 } else { 0.0 };
                snapshot.save();
                snapshot.translate(&graphene::Point::new(x, y));
                snapshot.append_layout(&layout, &fg);
                snapshot.restore();
            }
        }
    }
}

glib::wrapper! {
    pub struct Chart(ObjectSubclass<imp::Chart>) @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Chart {
    /// A chart for a metric (`cpu`, `memory`, `disk`, `network`).
    pub fn new(metric: &str, capacity: usize, span: &str) -> Self {
        let c: Self = glib::Object::new();
        c.add_css_class(metric);
        c.imp().capacity.set(capacity);
        c.imp().span.replace(span.to_owned());
        c.set_hexpand(true);
        c
    }

    pub fn set_format(&self, f: impl Fn(f64) -> String + 'static) {
        self.imp().format.replace(Some(Box::new(f)));
    }

    pub fn set_max(&self, max: Option<f64>) {
        self.imp().max.set(max);
    }

    pub fn set_floor(&self, floor: f64) {
        self.imp().floor.set(floor);
    }

    pub fn set_span(&self, span: &str) {
        self.imp().span.replace(span.to_owned());
        self.queue_draw();
    }

    /// Replaces the data: one or two series, oldest first.
    pub fn set_series(&self, series: Vec<Vec<f64>>) {
        self.imp().series.replace(series);
        self.queue_draw();
    }
}

#[cfg(test)]
mod tests {
    use super::nice_max;

    #[test]
    fn nice_scales() {
        assert_eq!(nice_max(0.0), 1.0);
        assert_eq!(nice_max(3.2), 5.0);
        assert_eq!(nice_max(12.0), 20.0);
        assert_eq!(nice_max(70_000.0), 100_000.0);
        assert_eq!(nice_max(150.0), 200.0);
    }
}
