//! cargo run --release --example asio_probe [plain]
//! What cpal's ASIO host reports per driver, on an STA thread like the engine's
//! audio-control thread (or a plain thread with `plain`). Windows only.

#[cfg(windows)]
fn main() {
    use cpal::traits::{DeviceTrait, HostTrait};

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(reserved: *mut std::ffi::c_void, coinit: u32) -> i32;
    }

    fn probe(tag: &str) {
        let host = cpal::host_from_id(cpal::HostId::Asio).expect("asio host");
        for dev in host.devices().expect("devices") {
            let name = dev.name().unwrap_or_default();
            let ins = dev.supported_input_configs().map(|c| c.map(|r| r.channels()).max());
            let outs = dev.supported_output_configs().map(|c| c.map(|r| r.channels()).max());
            println!("[{tag}] {name:28} in={ins:?} out={outs:?}");
        }
    }

    // only one of the two per process: cpal's ASIO layer is a process-wide singleton
    if std::env::args().any(|a| a == "plain") {
        std::thread::spawn(|| probe("plain")).join().unwrap();
        return;
    }
    std::thread::spawn(|| {
        let hr = unsafe { CoInitializeEx(std::ptr::null_mut(), 0x2) };
        println!("[sta] CoInitializeEx -> {hr:#x}");
        probe("sta");
    })
    .join()
    .unwrap();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("asio_probe is Windows only");
}
