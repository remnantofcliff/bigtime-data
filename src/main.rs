#![warn(clippy::pedantic)]

use std::io::Write;

#[derive(Clone, Copy, Debug)]
#[repr(C, align(16))]
pub struct Curve {
    p0: glam::Vec2,
    p1: glam::Vec2,
    p2: glam::Vec2,
    flags: u32,
}

impl Curve {
    #[must_use]
    pub fn with_p0(self, p0: glam::Vec2) -> Self {
        Self { p0, ..self }
    }
    #[must_use]
    pub fn with_p1(self, p1: glam::Vec2) -> Self {
        Self { p1, ..self }
    }
    #[must_use]
    pub fn with_p2(self, p2: glam::Vec2) -> Self {
        Self { p2, ..self }
    }
    #[must_use]
    pub fn set_line_flag(self) -> Self {
        Self {
            flags: self.flags | 1,
            ..self
        }
    }
    #[must_use]
    pub fn zeroed() -> Self {
        Self {
            p0: glam::Vec2::ZERO,
            p1: glam::Vec2::ZERO,
            p2: glam::Vec2::ZERO,
            flags: 0,
        }
    }
}

#[derive(Debug, Default)]
pub struct Outline {
    curves: Vec<Curve>,
}

impl Outline {
    fn process(&mut self, bbox: ttf_parser::Rect) {
        let bbox_min = glam::vec2(f32::from(bbox.x_min), f32::from(bbox.y_min));
        let bbox_size = glam::vec2(f32::from(bbox.width()), f32::from(bbox.height()));

        for curve in &mut self.curves {
            // Normalize curves to range (0, 1)
            curve.p0 -= bbox_min;
            curve.p1 -= bbox_min;
            curve.p2 -= bbox_min;
            curve.p0 /= bbox_size;
            curve.p1 /= bbox_size;
            curve.p2 /= bbox_size;

            // Invert curves and swap first and last point to correct winding order.
            curve.p0.y = 1.0 - curve.p0.y;
            curve.p1.y = 1.0 - curve.p1.y;
            curve.p2.y = 1.0 - curve.p2.y;
            std::mem::swap(&mut curve.p0, &mut curve.p2);
        }
    }
}

impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.curves.push(Curve::zeroed().with_p0(glam::vec2(x, y)));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let last = self.curves.last_mut().unwrap();
        *last = last
            .with_p1(glam::vec2((last.p0[0] + x) * 0.5, (last.p0[1] + y) * 0.5))
            .with_p2(glam::vec2(x, y))
            .set_line_flag();
        self.curves.push(Curve::zeroed().with_p0(glam::vec2(x, y)));
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let last = self.curves.last_mut().unwrap();
        *last = last.with_p1(glam::vec2(x1, y1)).with_p2(glam::vec2(x, y));
        self.curves.push(Curve::zeroed().with_p0(glam::vec2(x, y)));
    }

    fn curve_to(&mut self, _x1: f32, _y1: f32, _x2: f32, _y2: f32, _x: f32, _y: f32) {
        panic!("Cubic bezier!!");
    }

    #[allow(clippy::float_cmp)]
    fn close(&mut self) {
        assert!(
            self.curves.last().unwrap().p1 == glam::Vec2::ZERO
                && self.curves.last().unwrap().p2 == glam::Vec2::ZERO
        );
        self.curves.pop();
    }
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
struct Metrics {
    advance: f32,
}

///
/// Outline is in f32 0.0..=1.0.
/// All other values are in pixels
///
#[derive(Debug)]
struct GlyphData {
    outline: Option<Outline>,
    metrics: Metrics,
}

fn get_glyph_data(face: &ttf_parser::Face, c: char) -> Option<GlyphData> {
    let index = face.glyph_index(c)?;
    let global_bbox = face.global_bounding_box();
    let advance = face.glyph_hor_advance(index)?;
    let mut temp = Outline::default();
    let mut outline = face.outline_glyph(index, &mut temp).map(|_| temp);
    if let Some(outline) = &mut outline {
        outline.process(global_bbox);
    }
    let metrics = Metrics {
        advance: f32::from(advance) / f32::from(global_bbox.width()),
    };
    Some(GlyphData { outline, metrics })
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C, align(16))]
struct GlyphInfo {
    start: u32,
    end: u32,
}

fn write_buffer_data<T, P: AsRef<std::path::Path>>(buffer: &[T], file_name: P) {
    let mut file = std::fs::File::create(file_name).unwrap();

    let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast(), size_of_val(buffer)) };

    file.write_all(bytes).unwrap();
}

/// Iterate over u32s because char iterator skips invalid utf32 and in those cases we want to fill
/// stuff with default values
const RANGE: std::ops::RangeInclusive<u32> = char::MIN as u32..=char::MAX as u32;
const REPLACEMENT_CHARACTER: char = '\u{FFFD}';

fn main() -> std::process::ExitCode {
    let Some(dir_arg) = std::env::args().nth(1) else {
        println!("Pass output directory as first argument");
        return std::process::ExitCode::FAILURE;
    };

    let out_dir = std::path::Path::new(&dir_arg);

    if !out_dir.is_dir() {
        println!("Argument is not a directory");
        return std::process::ExitCode::FAILURE;
    }

    let data = include_bytes!("../Roboto-Regular.ttf");
    let face = ttf_parser::Face::parse(data, 0).unwrap();

    let mut map = std::collections::HashMap::new();
    for u in RANGE {
        if let Some(c) = char::from_u32(u) {
            if let Some(data) = get_glyph_data(&face, c) {
                map.insert(c, data);
            }
        }
    }

    let mut glyph_buffer = Vec::new();
    let mut info_buffer = Vec::new();
    let mut metrics_buffer = Vec::new();
    let mut replace = Vec::new();

    for u in RANGE {
        let mut info = GlyphInfo::default();
        let mut metrics = Metrics::default();
        let Some(c) = char::from_u32(u) else {
            metrics_buffer.push(metrics);
            info_buffer.push(info);
            replace.push(false);
            continue;
        };
        let should_replace_missing = c.is_alphanumeric();
        if let Some(data) = map.get(&c) {
            if let Some(outline) = &data.outline {
                info.start = u32::try_from(glyph_buffer.len()).unwrap();
                info.end = info.start + u32::try_from(outline.curves.len()).unwrap();
                glyph_buffer.extend_from_slice(&outline.curves);
            } else {
                info.start = 0;
                info.end = 0;
            }
            metrics = data.metrics;

            replace.push(false);
        } else {
            replace.push(should_replace_missing);
        }
        metrics_buffer.push(metrics);
        info_buffer.push(info);
    }

    for (i, should_replace) in replace.into_iter().enumerate() {
        if should_replace {
            info_buffer[i] = info_buffer[REPLACEMENT_CHARACTER as usize];
            metrics_buffer[i] = metrics_buffer[REPLACEMENT_CHARACTER as usize];
        }
    }

    println!(
        "Glyph buffer size: {} bytes\nInfo buffer size: {} bytes\nMetrics buffer size: {} bytes",
        size_of_val(glyph_buffer.as_slice()),
        size_of_val(info_buffer.as_slice()),
        size_of_val(metrics_buffer.as_slice())
    );

    write_buffer_data(&glyph_buffer, out_dir.join("glyph_buffer.data"));
    write_buffer_data(&info_buffer, out_dir.join("info_buffer.data"));
    write_buffer_data(&metrics_buffer, out_dir.join("metrics_buffer.data"));

    std::process::ExitCode::SUCCESS
}
