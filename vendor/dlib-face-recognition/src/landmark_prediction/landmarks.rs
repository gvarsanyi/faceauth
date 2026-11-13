use crate::geometry::Point;

cpp_class!(
    /// A wrapper around the dlib `full_object_detection` class, which internally has a `std::vector<point>`.
    /// https://github.com/davisking/dlib/blob/master/dlib/image_processing/full_object_detection.h#L21
    pub unsafe struct FaceLandmarks as "dlib::full_object_detection"
);

// faceauth patch: dlib 20 stores landmark parts as `dpoint` (double) while
// dlib 19 uses `point` (long), so a `&[Point]` view over the internal vector
// is not layout-compatible across versions. Expose per-part accessors that
// convert to `dlib::point` in C++ instead; both versions compile.
impl FaceLandmarks {
    /// Number of landmark points.
    pub fn len(&self) -> usize {
        unsafe {
            cpp!([self as "const dlib::full_object_detection*"] -> usize as "size_t" {
                return self->num_parts();
            })
        }
    }

    /// True if there are no landmark points.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Landmark point `index`, rounded to integer pixel coordinates.
    pub fn get(&self, index: usize) -> Option<Point> {
        if index >= self.len() {
            return None;
        }
        Some(unsafe {
            cpp!([self as "const dlib::full_object_detection*", index as "size_t"] -> Point as "dlib::point" {
                return dlib::point(self->part(index));
            })
        })
    }
}

#[test]
fn test_default_landmarks() {
    // ensure that FaceLandmarks::default() doesnt allow memory violations in safe code
    let landmarks = FaceLandmarks::default();

    assert!(landmarks.is_empty());
    assert_eq!(landmarks.len(), 0);
    assert_eq!(landmarks.get(0), None);
}
