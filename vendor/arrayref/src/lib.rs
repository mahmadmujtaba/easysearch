//! Build-time shim for the `arrayref` crate.
//!
//! Upstream `arrayref` 0.3.5–0.3.9 were yanked from crates.io (pulled in
//! transitively by eframe → winit → sctk-adwaita → tiny-skia), which breaks
//! fresh `cargo build`s. This is a small, original reimplementation of the
//! same macro surface — `array_ref!`, `array_refs!`, `array_mut_ref!`,
//! `mut_array_refs!` — so the dependency graph resolves without upstream
//! source. Upstream license: BSD-2-Clause; this shim is new code implementing
//! the same public API. Contract unchanged: the caller guarantees that the
//! requested slices are within bounds.

/// `&[T; N]` view of a `&[T]` slice. Caller must ensure `s.len() == N`.
#[inline]
pub unsafe fn extract_ref<const N: usize, T>(s: &[T]) -> &[T; N] {
    debug_assert_eq!(s.len(), N);
    &*(s.as_ptr() as *const [T; N])
}

/// `&mut [T; N]` view of a `&mut [T]` slice. Caller must ensure `s.len() == N`.
#[inline]
pub unsafe fn extract_mut_ref<const N: usize, T>(s: &mut [T]) -> &mut [T; N] {
    debug_assert_eq!(s.len(), N);
    &mut *(s.as_mut_ptr() as *mut [T; N])
}

/// Borrow a fixed-size array reference from a slice: `array_ref![s, 0, 4]`.
///
/// `$start` is evaluated before `$arr` so that call sites like
/// `array_ref![self.data, self.offset(x, y), N]` don't trip the borrow
/// checker (the immutable `self` borrow ends before the slice borrow begins).
#[macro_export]
macro_rules! array_ref {
    ($arr:expr, $start:expr, $len:expr) => {{
        let start = $start;
        let s = &$arr[start..(start + $len)];
        unsafe { $crate::extract_ref(s) }
    }};
}

/// Borrow several fixed-size array references from a slice:
/// `array_refs![s, 4, 4, 4]` → `(&[T;4], &[T;4], &[T;4])`.
#[macro_export]
macro_rules! array_refs {
    ($arr:expr, $($len:expr),+) => {{
        let arr = $arr;
        let mut ptr = arr.as_ptr();
        (
            $(
                {
                    let s = unsafe { std::slice::from_raw_parts(ptr, $len) };
                    ptr = unsafe { ptr.add($len) };
                    unsafe { $crate::extract_ref(s) }
                }
            ),+
        )
    }};
}

/// Mutably borrow a fixed-size array reference: `array_mut_ref![s, 0, 4]`.
///
/// `$start` is evaluated before `$arr` (see `array_ref!`).
#[macro_export]
macro_rules! array_mut_ref {
    ($arr:expr, $start:expr, $len:expr) => {{
        let start = $start;
        let s = &mut $arr[start..(start + $len)];
        unsafe { $crate::extract_mut_ref(s) }
    }};
}

/// Mutably borrow several fixed-size array references:
/// `mut_array_refs![s, 4, 4, 4]` → `(&mut [T;4], &mut [T;4], &mut [T;4])`.
#[macro_export]
macro_rules! mut_array_refs {
    ($arr:expr, $($len:expr),+) => {{
        let arr = $arr;
        let mut ptr = arr.as_mut_ptr();
        (
            $(
                {
                    let s = unsafe { std::slice::from_raw_parts_mut(ptr, $len) };
                    ptr = unsafe { ptr.add($len) };
                    unsafe { $crate::extract_mut_ref(s) }
                }
            ),+
        )
    }};
}
