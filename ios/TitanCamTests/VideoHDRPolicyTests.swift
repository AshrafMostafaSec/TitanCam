import XCTest
@testable import TitanCam

final class VideoHDRPolicyTests: XCTestCase {
    // Models the documented AVFoundation precondition without needing a camera
    // on a cloud simulator. Physical iPhone qualification is still required.
    private final class Device: VideoHDRControllable {
        var automaticallyAdjustsVideoHDREnabled = true
        var enabled: Bool
        var illegalWrites = 0
        var manualWrites = 0
        init(enabled: Bool) { self.enabled = enabled }
        var isVideoHDREnabled: Bool {
            get { enabled }
            set {
                if automaticallyAdjustsVideoHDREnabled { illegalWrites += 1 }
                manualWrites += 1
                enabled = newValue
            }
        }
    }

    func testAutomaticHDREnabledAtConnectionDoesNotViolateSetterPrecondition() {
        let device = Device(enabled: true)
        VideoHDRPolicy.applySDR(to: device)
        XCTAssertEqual(device.illegalWrites, 0)
        XCTAssertEqual(device.manualWrites, 1)
        XCTAssertFalse(device.automaticallyAdjustsVideoHDREnabled)
        XCTAssertFalse(device.isVideoHDREnabled)
    }

    func testAlreadySDRAndRepeatedConfigurationAvoidUnnecessaryManualHDRWrites() {
        let device = Device(enabled: false)
        VideoHDRPolicy.applySDR(to: device)
        VideoHDRPolicy.applySDR(to: device)
        XCTAssertEqual(device.illegalWrites, 0)
        XCTAssertEqual(device.manualWrites, 0)
        XCTAssertFalse(device.automaticallyAdjustsVideoHDREnabled)
        XCTAssertFalse(device.isVideoHDREnabled)
    }
}
