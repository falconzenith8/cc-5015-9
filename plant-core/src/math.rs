//! Minimal deterministic math primitives. No floating-point environment state
//! is used anywhere; all results depend only on the inputs.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3::new(0.0, 0.0, 0.0);
    pub const UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }

    pub fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }

    pub fn scale(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }

    pub fn mul(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x * o.x, self.y * o.y, self.z * o.z)
    }

    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }

    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    pub fn normalize(self) -> Vec3 {
        let len = self.length();
        if len <= f32::MIN_POSITIVE {
            return Vec3::ZERO;
        }
        self.scale(1.0 / len)
    }

    /// Safe normalization used by frame construction: a degenerate input
    /// collapses to `fallback` instead of producing NaN vectors.
    pub fn normalize_or(self, fallback: Vec3) -> Vec3 {
        let len = self.length();
        if len <= f32::MIN_POSITIVE || !len.is_finite() {
            fallback
        } else {
            self.scale(1.0 / len)
        }
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    pub const fn to_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    pub const IDENTITY: Quat = Quat::new(0.0, 0.0, 0.0, 1.0);

    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    /// Rotation mapping unit vector `from` onto unit vector `to`, handling the
    /// antiparallel case with a deterministic fallback axis.
    pub fn rotation_arc(from: Vec3, to: Vec3) -> Quat {
        let f = from.normalize_or(Vec3::UP);
        let t = to.normalize_or(Vec3::UP);
        let d = f.dot(t).clamp(-1.0, 1.0);
        if d > 1.0 - 1e-6 {
            return Quat::IDENTITY;
        }
        if d < -1.0 + 1e-6 {
            // 180 degree turn: pick any axis orthogonal to `from`,
            // deterministically so the same inputs always give the same quat.
            let axis = if f.x.abs() < 0.9 {
                Vec3::new(1.0, 0.0, 0.0).cross(f)
            } else {
                Vec3::new(0.0, 1.0, 0.0).cross(f)
            }
            .normalize_or(Vec3::UP);
            return Quat::new(axis.x, axis.y, axis.z, 0.0);
        }
        let s = f.cross(t);
        let q = Quat::new(s.x, s.y, s.z, 1.0 + d);
        let n = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
        Quat::new(q.x / n, q.y / n, q.z / n, q.w / n)
    }

    /// Rotation of `angle` radians around the (normalized) axis `axis`.
    pub fn axis_angle(axis: Vec3, angle: f32) -> Quat {
        let a = axis.normalize_or(Vec3::UP);
        let half = angle * 0.5;
        let s = half.sin();
        Quat::new(a.x * s, a.y * s, a.z * s, half.cos())
    }

    pub fn rotate_vec(self, v: Vec3) -> Vec3 {
        // q * v * q^-1 expanded for unit quaternions.
        let q = Vec3::new(self.x, self.y, self.z);
        let t = q.cross(v).scale(2.0);
        v.add(t.scale(self.w)).add(q.cross(t))
    }

    pub fn mul(self, o: Quat) -> Quat {
        Quat::new(
            self.w * o.x + self.x * o.w + self.y * o.z - self.z * o.y,
            self.w * o.y - self.x * o.z + self.y * o.w + self.z * o.x,
            self.w * o.z + self.x * o.y - self.y * o.x + self.z * o.w,
            self.w * o.w - self.x * o.x - self.y * o.y - self.z * o.z,
        )
    }

    pub fn normalize(self) -> Quat {
        let n = (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt();
        if n <= f32::MIN_POSITIVE {
            return Quat::IDENTITY;
        }
        Quat::new(self.x / n, self.y / n, self.z / n, self.w / n)
    }
}

/// Column-major 4x4 transform, composed as translation * rotation * uniform scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4 {
    pub cols: [Vec3; 3],
    pub translation: Vec3,
    pub scale: f32,
}

impl Mat4 {
    pub const IDENTITY: Mat4 = Mat4 {
        cols: [Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)],
        translation: Vec3::ZERO,
        scale: 1.0,
    };

    pub fn from_trs(translation: Vec3, rotation: Quat, scale: f32) -> Self {
        let x = rotation.rotate_vec(Vec3::new(1.0, 0.0, 0.0));
        let y = rotation.rotate_vec(Vec3::new(0.0, 1.0, 0.0));
        let z = rotation.rotate_vec(Vec3::new(0.0, 0.0, 1.0));
        Self {
            cols: [x, y, z],
            translation,
            scale,
        }
    }

    pub fn mul_point(self, p: Vec3) -> Vec3 {
        let scaled = Vec3::new(
            p.x * self.cols[0].x + p.y * self.cols[1].x + p.z * self.cols[2].x,
            p.x * self.cols[0].y + p.y * self.cols[1].y + p.z * self.cols[2].y,
            p.x * self.cols[0].z + p.y * self.cols[1].z + p.z * self.cols[2].z,
        )
        .scale(self.scale);
        scaled.add(self.translation)
    }

    /// Parent-child composition: `parent * child`.
    pub fn mul(self, child: Mat4) -> Mat4 {
        // Rotation columns stay unit-length; uniform scale is tracked separately
        // so a scaled parent cannot skew its children's orientation.
        let col = |i: usize| self.mul_direction(child.cols[i]);
        Mat4 {
            cols: [col(0), col(1), col(2)],
            translation: self.mul_point(child.translation),
            scale: self.scale * child.scale,
        }
    }

    fn mul_direction(self, d: Vec3) -> Vec3 {
        Vec3::new(
            d.x * self.cols[0].x + d.y * self.cols[1].x + d.z * self.cols[2].x,
            d.x * self.cols[0].y + d.y * self.cols[1].y + d.z * self.cols[2].y,
            d.x * self.cols[0].z + d.y * self.cols[1].z + d.z * self.cols[2].z,
        )
    }
}
