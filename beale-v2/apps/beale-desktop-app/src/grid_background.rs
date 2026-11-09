use rune::{Color, Shape};

pub(crate) fn dots(width: f32, height: f32, color: Color) -> Shape {
    let mut shape = Shape::new();
    for x in (8..width as usize).step_by(16) {
        for y in (8..height as usize).step_by(16) {
            let (x, y) = (x as f32, y as f32);
            shape = shape.fill(
                vec![[x, y], [x + 1.0, y], [x + 1.0, y + 1.0], [x, y + 1.0]],
                color,
            );
        }
    }
    shape
}
