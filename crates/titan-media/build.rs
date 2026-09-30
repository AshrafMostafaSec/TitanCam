fn main() {
    let out = std::process::Command::new("pkg-config")
        .args(["--cflags", "libpipewire-0.3"])
        .output()
        .expect("pkg-config required");
    assert!(out.status.success(), "PipeWire development headers missing");
    let mut c = cc::Build::new();
    c.file("src/mic.c").flag("-std=c11");
    for flag in String::from_utf8(out.stdout).unwrap().split_whitespace() {
        c.flag(flag);
    }
    c.compile("titan_mic");
    println!("cargo:rustc-link-lib=pipewire-0.3");
    println!("cargo:rustc-link-lib=opus");
    println!("cargo:rerun-if-changed=src/mic.c");
}
