import Foundation
import VideoToolbox

enum HardwareEncoderPolicy {
    // Only called after successful Create with RequireHardware=true and Prepare.
    // Some encoders do not expose the optional property. Apple's mandatory
    // creation contract still excludes software in that case.
    static func evidence(requiredHardware: Bool, queryStatus: OSStatus, value: CFTypeRef?) throws -> String {
        guard requiredHardware else { throw CameraError.unavailable("Hardware must be required when creating the encoder") }
        if queryStatus == kVTPropertyNotSupportedErr { return "required-hardware-session" }
        guard queryStatus == noErr else { throw CameraError.unavailable("Hardware encoder query failed (\(queryStatus))") }
        if value == nil { return "required-hardware-session-no-property-value" }
        guard let value, CFGetTypeID(value) == CFBooleanGetTypeID() else {
            throw CameraError.unavailable("Hardware encoder returned an invalid confirmation value")
        }
        guard CFEqual(value, kCFBooleanTrue) else { throw CameraError.unavailable("Encoder explicitly reported software encoding") }
        return "hardware-property"
    }
}
