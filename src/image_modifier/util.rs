pub fn aspect(width: i32, height: i32) -> f64 {
  width.max(height) as f64 / width.min(height) as f64
}
