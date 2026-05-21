/*
* This Source Code Form is subject to the terms of the Mozilla Public
* License, v. 2.0. If a copy of the MPL was not distributed with this
* file, You can obtain one at https://mozilla.org/MPL/2.0/.
*/
//! `CATransform3D` implementation for Core Animation.

use crate::abi::{impl_GuestRet_for_large_struct, GuestArg};
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::core_graphics::cg_affine_transform::CGAffineTransform;
use crate::matrix::Matrix;
use crate::mem::SafeRead;
use crate::Environment;

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C, packed)]
pub struct CATransform3D {
   pub m11: CGFloat, pub m12: CGFloat, pub m13: CGFloat, pub m14: CGFloat,
   pub m21: CGFloat, pub m22: CGFloat, pub m23: CGFloat, pub m24: CGFloat,
   pub m31: CGFloat, pub m32: CGFloat, pub m33: CGFloat, pub m34: CGFloat,
   pub m41: CGFloat, pub m42: CGFloat, pub m43: CGFloat, pub m44: CGFloat,
}
unsafe impl SafeRead for CATransform3D {}

impl GuestArg for CATransform3D {
   const REG_COUNT: usize = 16;
   fn from_regs(regs: &[u32]) -> Self {
       CATransform3D {
           m11: GuestArg::from_regs(&regs[0..1]),
           m12: GuestArg::from_regs(&regs[1..2]),
           m13: GuestArg::from_regs(&regs[2..3]),
           m14: GuestArg::from_regs(&regs[3..4]),
           m21: GuestArg::from_regs(&regs[4..5]),
           m22: GuestArg::from_regs(&regs[5..6]),
           m23: GuestArg::from_regs(&regs[6..7]),
           m24: GuestArg::from_regs(&regs[7..8]),
           m31: GuestArg::from_regs(&regs[8..9]),
           m32: GuestArg::from_regs(&regs[9..10]),
           m33: GuestArg::from_regs(&regs[10..11]),
           m34: GuestArg::from_regs(&regs[11..12]),
           m41: GuestArg::from_regs(&regs[12..13]),
           m42: GuestArg::from_regs(&regs[13..14]),
           m43: GuestArg::from_regs(&regs[14..15]),
           m44: GuestArg::from_regs(&regs[15..16]),
       }
   }
   fn to_regs(self, regs: &mut [u32]) {
       // Copy fields to locals to avoid unaligned reference (E0793)
       let (m11,m12,m13,m14) = (self.m11,self.m12,self.m13,self.m14);
       let (m21,m22,m23,m24) = (self.m21,self.m22,self.m23,self.m24);
       let (m31,m32,m33,m34) = (self.m31,self.m32,self.m33,self.m34);
       let (m41,m42,m43,m44) = (self.m41,self.m42,self.m43,self.m44);
       m11.to_regs(&mut regs[0..1]);   m12.to_regs(&mut regs[1..2]);
       m13.to_regs(&mut regs[2..3]);   m14.to_regs(&mut regs[3..4]);
       m21.to_regs(&mut regs[4..5]);   m22.to_regs(&mut regs[5..6]);
       m23.to_regs(&mut regs[6..7]);   m24.to_regs(&mut regs[7..8]);
       m31.to_regs(&mut regs[8..9]);   m32.to_regs(&mut regs[9..10]);
       m33.to_regs(&mut regs[10..11]); m34.to_regs(&mut regs[11..12]);
       m41.to_regs(&mut regs[12..13]); m42.to_regs(&mut regs[13..14]);
       m43.to_regs(&mut regs[14..15]); m44.to_regs(&mut regs[15..16]);
   }
}
impl_GuestRet_for_large_struct!(CATransform3D);

// MARK: - Matrix conversions

impl From<CATransform3D> for Matrix<4> {
   fn from(value: CATransform3D) -> Matrix<4> {
       // Copy packed fields to locals first.
       let (m11,m12,m13,m14) = (value.m11,value.m12,value.m13,value.m14);
       let (m21,m22,m23,m24) = (value.m21,value.m22,value.m23,value.m24);
       let (m31,m32,m33,m34) = (value.m31,value.m32,value.m33,value.m34);
       let (m41,m42,m43,m44) = (value.m41,value.m42,value.m43,value.m44);
       Matrix::<4>::from_columns([
           [m11, m12, m13, m14],
           [m21, m22, m23, m24],
           [m31, m32, m33, m34],
           [m41, m42, m43, m44],
       ])
   }
}

impl From<Matrix<4>> for CATransform3D {
   fn from(matrix: Matrix<4>) -> Self {
       let c = matrix.columns();
       CATransform3D {
           m11: c[0][0], m12: c[0][1], m13: c[0][2], m14: c[0][3],
           m21: c[1][0], m22: c[1][1], m23: c[1][2], m24: c[1][3],
           m31: c[2][0], m32: c[2][1], m33: c[2][2], m34: c[2][3],
           m41: c[3][0], m42: c[3][1], m43: c[3][2], m44: c[3][3],
       }
   }
}

// MARK: - Identity constant

#[rustfmt::skip]
pub const CATransform3DIdentity: CATransform3D = CATransform3D {
   m11: 1.0, m12: 0.0, m13: 0.0, m14: 0.0,
   m21: 0.0, m22: 1.0, m23: 0.0, m24: 0.0,
   m31: 0.0, m32: 0.0, m33: 1.0, m34: 0.0,
   m41: 0.0, m42: 0.0, m43: 0.0, m44: 1.0,
};

pub const CONSTANTS: ConstantExports = &[(
   "_CATransform3DIdentity",
   HostConstant::Custom(|env| {
       env.mem.alloc_and_write(CATransform3DIdentity).cast().cast_const()
   }),
)];

// MARK: - impl CATransform3D

impl CATransform3D {
   pub fn is_identity(self) -> bool {
       // Compare field by field using locals (avoids packed-ref UB).
       let id = CATransform3DIdentity;
       let (a11,a12,a13,a14) = (self.m11,self.m12,self.m13,self.m14);
       let (a21,a22,a23,a24) = (self.m21,self.m22,self.m23,self.m24);
       let (a31,a32,a33,a34) = (self.m31,self.m32,self.m33,self.m34);
       let (a41,a42,a43,a44) = (self.m41,self.m42,self.m43,self.m44);
       a11==id.m11 && a12==id.m12 && a13==id.m13 && a14==id.m14
           && a21==id.m21 && a22==id.m22 && a23==id.m23 && a24==id.m24
           && a31==id.m31 && a32==id.m32 && a33==id.m33 && a34==id.m34
           && a41==id.m41 && a42==id.m42 && a43==id.m43 && a44==id.m44
   }

   pub fn make_translation(tx: CGFloat, ty: CGFloat, tz: CGFloat) -> Self {
       let mut t = CATransform3DIdentity;
       t.m41 = tx; t.m42 = ty; t.m43 = tz;
       t
   }

   pub fn make_scale(sx: CGFloat, sy: CGFloat, sz: CGFloat) -> Self {
       let mut t = CATransform3DIdentity;
       t.m11 = sx; t.m22 = sy; t.m33 = sz;
       t
   }

   pub fn make_rotation(angle: CGFloat, x: CGFloat, y: CGFloat, z: CGFloat) -> Self {
       let length = (x * x + y * y + z * z).sqrt();
       if length == 0.0 { return CATransform3DIdentity; }
       let nx = x / length;
       let ny = y / length;
       let nz = z / length;
       let c = angle.cos();
       let s = angle.sin();
       let t = 1.0 - c;
       CATransform3D {
           m11: t*nx*nx + c,       m12: t*nx*ny + nz*s,   m13: t*nx*nz - ny*s,   m14: 0.0,
           m21: t*nx*ny - nz*s,    m22: t*ny*ny + c,       m23: t*ny*nz + nx*s,   m24: 0.0,
           m31: t*nx*nz + ny*s,    m32: t*ny*nz - nx*s,    m33: t*nz*nz + c,       m34: 0.0,
           m41: 0.0,               m42: 0.0,               m43: 0.0,               m44: 1.0,
       }
   }

   pub fn concat(self, other: Self) -> Self {
       let a: Matrix<4> = self.into();
       let b: Matrix<4> = other.into();
       Matrix::<4>::multiply(&a, &b).into()
   }

   pub fn rotate(self, angle: CGFloat, x: CGFloat, y: CGFloat, z: CGFloat) -> Self {
       Self::make_rotation(angle, x, y, z).concat(self)
   }

   pub fn scale(self, sx: CGFloat, sy: CGFloat, sz: CGFloat) -> Self {
       Self::make_scale(sx, sy, sz).concat(self)
   }

   pub fn translate(self, tx: CGFloat, ty: CGFloat, tz: CGFloat) -> Self {
       Self::make_translation(tx, ty, tz).concat(self)
   }

pub fn invert(self) -> Option<Self> {
    // Matrix<4> has no inverse() — compute it directly via
    // cofactor expansion (Cramer's rule on the 4×4 matrix).
    let (m11,m12,m13,m14) = (self.m11,self.m12,self.m13,self.m14);
    let (m21,m22,m23,m24) = (self.m21,self.m22,self.m23,self.m24);
    let (m31,m32,m33,m34) = (self.m31,self.m32,self.m33,self.m34);
    let (m41,m42,m43,m44) = (self.m41,self.m42,self.m43,self.m44);

    // 2×2 sub-determinants used repeatedly below.
    let s0  = m11*m22 - m21*m12;
    let s1  = m11*m23 - m21*m13;
    let s2  = m11*m24 - m21*m14;
    let s3  = m12*m23 - m22*m13;
    let s4  = m12*m24 - m22*m14;
    let s5  = m13*m24 - m23*m14;
    let c5  = m33*m44 - m43*m34;
    let c4  = m32*m44 - m42*m34;
    let c3  = m32*m43 - m42*m33;
    let c2  = m31*m44 - m41*m34;
    let c1  = m31*m43 - m41*m33;
    let c0  = m31*m42 - m41*m32;

    let det = s0*c5 - s1*c4 + s2*c3 + s3*c2 - s4*c1 + s5*c0;
    if det.abs() < f32::EPSILON {
        return None; // singular
    }
    let inv_det = 1.0 / det;

    Some(CATransform3D {
        m11: ( m22*c5 - m23*c4 + m24*c3) * inv_det,
        m12: (-m12*c5 + m13*c4 - m14*c3) * inv_det,
        m13: ( m42*s5 - m43*s4 + m44*s3) * inv_det,
        m14: (-m32*s5 + m33*s4 - m34*s3) * inv_det,

        m21: (-m21*c5 + m23*c2 - m24*c1) * inv_det,
        m22: ( m11*c5 - m13*c2 + m14*c1) * inv_det,
        m23: (-m41*s5 + m43*s2 - m44*s1) * inv_det,
        m24: ( m31*s5 - m33*s2 + m34*s1) * inv_det,

        m31: ( m21*c4 - m22*c2 + m24*c0) * inv_det,
        m32: (-m11*c4 + m12*c2 - m14*c0) * inv_det,
        m33: ( m41*s4 - m42*s2 + m44*s0) * inv_det,
        m34: (-m31*s4 + m32*s2 - m34*s0) * inv_det,

        m41: (-m21*c3 + m22*c1 - m23*c0) * inv_det,
        m42: ( m11*c3 - m12*c1 + m13*c0) * inv_det,
        m43: (-m41*s3 + m42*s1 - m43*s0) * inv_det,
        m44: ( m31*s3 - m32*s1 + m33*s0) * inv_det,
    })
}

   /// Convert from a 2D CGAffineTransform (extends into 3D with zero z).
   pub fn from_affine(t: CGAffineTransform) -> Self {
       // Copy packed fields.
       let (a, b, c, d, tx, ty) = (t.a, t.b, t.c, t.d, t.tx, t.ty);
       CATransform3D {
           m11: a,   m12: b,   m13: 0.0, m14: 0.0,
           m21: c,   m22: d,   m23: 0.0, m24: 0.0,
           m31: 0.0, m32: 0.0, m33: 1.0, m34: 0.0,
           m41: tx,  m42: ty,  m43: 0.0, m44: 1.0,
       }
   }

   /// Project to 2D CGAffineTransform (valid only when z components are identity).
   pub fn to_affine(self) -> Option<CGAffineTransform> {
       let (m13,m14,m23,m24,m34,m43) = (self.m13,self.m14,self.m23,self.m24,self.m34,self.m43);
       if m13 != 0.0 || m14 != 0.0 || m23 != 0.0 || m24 != 0.0
           || m34 != 0.0 || m43 != 0.0
       {
           return None;
       }
       Some(CGAffineTransform {
           a: self.m11, b: self.m12,
           c: self.m21, d: self.m22,
           tx: self.m41, ty: self.m42,
       })
   }
}

// MARK: - Free functions (C API)

fn CATransform3DIsIdentity(_env: &mut Environment, t: CATransform3D) -> bool {
   t.is_identity()
}

fn CATransform3DEqualToTransform(_env: &mut Environment, a: CATransform3D, b: CATransform3D) -> bool {
   // Compare via locals to avoid packed-ref issues.
   let (a11,a12,a13,a14) = (a.m11,a.m12,a.m13,a.m14);
   let (a21,a22,a23,a24) = (a.m21,a.m22,a.m23,a.m24);
   let (a31,a32,a33,a34) = (a.m31,a.m32,a.m33,a.m34);
   let (a41,a42,a43,a44) = (a.m41,a.m42,a.m43,a.m44);
   let (b11,b12,b13,b14) = (b.m11,b.m12,b.m13,b.m14);
   let (b21,b22,b23,b24) = (b.m21,b.m22,b.m23,b.m24);
   let (b31,b32,b33,b34) = (b.m31,b.m32,b.m33,b.m34);
   let (b41,b42,b43,b44) = (b.m41,b.m42,b.m43,b.m44);
   a11==b11 && a12==b12 && a13==b13 && a14==b14
       && a21==b21 && a22==b22 && a23==b23 && a24==b24
       && a31==b31 && a32==b32 && a33==b33 && a34==b34
       && a41==b41 && a42==b42 && a43==b43 && a44==b44
}

fn CATransform3DMakeTranslation(
   _env: &mut Environment,
   tx: CGFloat, ty: CGFloat, tz: CGFloat,
) -> CATransform3D {
   CATransform3D::make_translation(tx, ty, tz)
}

fn CATransform3DMakeScale(
   _env: &mut Environment,
   sx: CGFloat, sy: CGFloat, sz: CGFloat,
) -> CATransform3D {
   CATransform3D::make_scale(sx, sy, sz)
}

fn CATransform3DMakeRotation(
   _env: &mut Environment,
   angle: CGFloat, x: CGFloat, y: CGFloat, z: CGFloat,
) -> CATransform3D {
   CATransform3D::make_rotation(angle, x, y, z)
}

fn CATransform3DTranslate(
   _env: &mut Environment,
   t: CATransform3D,
   tx: CGFloat, ty: CGFloat, tz: CGFloat,
) -> CATransform3D {
   t.translate(tx, ty, tz)
}

fn CATransform3DScale(
   _env: &mut Environment,
   t: CATransform3D,
   sx: CGFloat, sy: CGFloat, sz: CGFloat,
) -> CATransform3D {
   t.scale(sx, sy, sz)
}

fn CATransform3DRotate(
   _env: &mut Environment,
   t: CATransform3D,
   angle: CGFloat, x: CGFloat, y: CGFloat, z: CGFloat,
) -> CATransform3D {
   t.rotate(angle, x, y, z)
}

fn CATransform3DConcat(
   _env: &mut Environment,
   a: CATransform3D,
   b: CATransform3D,
) -> CATransform3D {
   a.concat(b)
}

fn CATransform3DInvert(
    _env: &mut Environment,
    t: CATransform3D,
) -> CATransform3D {
    t.invert().unwrap_or(t)   // unchanged — already correct
}


fn CATransform3DMakeAffineTransform(
   _env: &mut Environment,
   affine: CGAffineTransform,
) -> CATransform3D {
   CATransform3D::from_affine(affine)
}

fn CATransform3DIsAffine(
   _env: &mut Environment,
   t: CATransform3D,
) -> bool {
   t.to_affine().is_some()
}

fn CATransform3DGetAffineTransform(
   _env: &mut Environment,
   t: CATransform3D,
) -> CGAffineTransform {
   t.to_affine().unwrap_or(crate::frameworks::core_graphics::cg_affine_transform::CGAffineTransformIdentity)
}

// MARK: - String representation helpers

fn CATransform3DFromString(env: &mut Environment, string: crate::objc::id) -> CATransform3D {
   if string == crate::objc::nil { return CATransform3DIdentity; }
   let s = crate::frameworks::foundation::ns_string::to_rust_string(env, string).into_owned();
   // Format: "[m11 m12 m13 m14; m21 m22 m23 m24; m31 m32 m33 m34; m41 m42 m43 m44]"
   let nums: Vec<f32> = s
       .trim_matches(|c| c == '[' || c == ']')
       .split(|c| c == ' ' || c == ',' || c == ';')
       .filter_map(|t| t.trim().parse::<f32>().ok())
       .collect();
   if nums.len() >= 16 {
       CATransform3D {
           m11: nums[0],  m12: nums[1],  m13: nums[2],  m14: nums[3],
           m21: nums[4],  m22: nums[5],  m23: nums[6],  m24: nums[7],
           m31: nums[8],  m32: nums[9],  m33: nums[10], m34: nums[11],
           m41: nums[12], m42: nums[13], m43: nums[14], m44: nums[15],
       }
   } else {
       log!("CATransform3DFromString: couldn't parse {:?}", s);
       CATransform3DIdentity
   }
}

fn CATransform3DToString(env: &mut Environment, t: CATransform3D) -> crate::objc::id {
   // Copy packed fields.
   let (m11,m12,m13,m14) = (t.m11,t.m12,t.m13,t.m14);
   let (m21,m22,m23,m24) = (t.m21,t.m22,t.m23,t.m24);
   let (m31,m32,m33,m34) = (t.m31,t.m32,t.m33,t.m34);
   let (m41,m42,m43,m44) = (t.m41,t.m42,t.m43,t.m44);
   let s = format!(
       "[{} {} {} {}; {} {} {} {}; {} {} {} {}; {} {} {} {}]",
       m11,m12,m13,m14, m21,m22,m23,m24,
       m31,m32,m33,m34, m41,m42,m43,m44
   );
   let ns = crate::frameworks::foundation::ns_string::from_rust_string(env, s);
   crate::objc::autorelease(env, ns)
}

pub const FUNCTIONS: FunctionExports = &[
   // Identity check / equality
   export_c_func!(CATransform3DIsIdentity(_)),
   export_c_func!(CATransform3DEqualToTransform(_, _)),
   // Make constructors
   export_c_func!(CATransform3DMakeTranslation(_, _, _)),
   export_c_func!(CATransform3DMakeScale(_, _, _)),
   export_c_func!(CATransform3DMakeRotation(_, _, _, _)),
   // Modifier functions
   export_c_func!(CATransform3DTranslate(_, _, _, _)),
   export_c_func!(CATransform3DScale(_, _, _, _)),
   export_c_func!(CATransform3DRotate(_, _, _, _, _)),
   export_c_func!(CATransform3DConcat(_, _)),
   export_c_func!(CATransform3DInvert(_)),
   // Affine bridge
   export_c_func!(CATransform3DMakeAffineTransform(_)),
   export_c_func!(CATransform3DIsAffine(_)),
   export_c_func!(CATransform3DGetAffineTransform(_)),
   // String
   export_c_func!(CATransform3DFromString(_)),
   export_c_func!(CATransform3DToString(_)),
];
