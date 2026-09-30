//! C shim owns the ABI; Rust owns successful connection file descriptors.
use std::{
    ffi::CString,
    os::fd::{FromRawFd, OwnedFd},
};
unsafe extern "C" {
    fn titan_usb_devices(out: *mut libc::c_char, capacity: libc::c_int) -> libc::c_int;
    fn titan_usb_connect(udid: *const libc::c_char, port: u16) -> libc::c_int;
}
pub fn devices() -> anyhow::Result<Vec<String>> {
    let mut b = vec![0u8; 4096]; // SAFETY: writable allocation and size agree; C copies bounded bytes.
    let n = unsafe { titan_usb_devices(b.as_mut_ptr().cast(), b.len() as i32) };
    anyhow::ensure!(n >= 0, "usbmuxd enumeration failed");
    Ok(String::from_utf8(b[..n as usize].to_vec())?
        .lines()
        .map(String::from)
        .collect())
}
pub fn connect(udid: &str, port: u16) -> anyhow::Result<OwnedFd> {
    let id = CString::new(udid)?; // SAFETY: C consumes a NUL-terminated string synchronously, returning a new owned FD.
    let fd = unsafe { titan_usb_connect(id.as_ptr(), port) };
    anyhow::ensure!(
        fd >= 0,
        "iPhone port {port} unavailable; launch TitanCam and enable USB"
    ); // SAFETY: successful libusbmuxd connection uniquely transfers descriptor ownership.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}
