import XCTest
import VideoToolbox
@testable import TitanCam

final class HardwareEncoderPolicyTests: XCTestCase {
    func testConfirmedHardwareAndUnsupportedOptionalQueryWithMandatoryHardware() throws {
        XCTAssertEqual(try HardwareEncoderPolicy.evidence(requiredHardware: true, queryStatus: noErr, value: kCFBooleanTrue), "hardware-property")
        XCTAssertEqual(try HardwareEncoderPolicy.evidence(requiredHardware: true, queryStatus: kVTPropertyNotSupportedErr, value: nil), "required-hardware-session")
        XCTAssertEqual(try HardwareEncoderPolicy.evidence(requiredHardware: true, queryStatus: noErr, value: nil), "required-hardware-session-no-property-value")
    }
    func testSoftwareInvalidValuesAndUnexpectedQueryErrorsAreRejected() {
        XCTAssertThrowsError(try HardwareEncoderPolicy.evidence(requiredHardware: true, queryStatus: noErr, value: kCFBooleanFalse))
        XCTAssertThrowsError(try HardwareEncoderPolicy.evidence(requiredHardware: true, queryStatus: noErr, value: NSNumber(value: 1)))
        XCTAssertThrowsError(try HardwareEncoderPolicy.evidence(requiredHardware: true, queryStatus: -12345, value: nil))
        XCTAssertThrowsError(try HardwareEncoderPolicy.evidence(requiredHardware: false, queryStatus: kVTPropertyNotSupportedErr, value: nil))
    }
}
