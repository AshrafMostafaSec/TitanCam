fn main() {
    cc::Build::new().file("src/usb.c").compile("titan_usb_shim");
    println!("cargo:rustc-link-lib=usbmuxd-2.0");
    println!("cargo:rerun-if-changed=src/usb.c");
}
