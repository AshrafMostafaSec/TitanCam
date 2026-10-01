import AVFoundation

// AVCaptureDevice's HDR setter raises an Objective-C exception while automatic
// HDR is enabled. Swift do/catch cannot handle that exception. Keep the Apple
// API precondition explicit and shared with the regression test.
protocol VideoHDRControllable: AnyObject {
    var automaticallyAdjustsVideoHDREnabled: Bool { get set }
    var isVideoHDREnabled: Bool { get set }
}

extension AVCaptureDevice: VideoHDRControllable {}

enum VideoHDRPolicy {
    // Caller must hold the device configuration lock, within a session transaction.
    static func applySDR(to device: VideoHDRControllable) {
        device.automaticallyAdjustsVideoHDREnabled = false
        if device.isVideoHDREnabled { device.isVideoHDREnabled = false }
    }
}
