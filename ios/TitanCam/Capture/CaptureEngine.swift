import Foundation
import AVFoundation

final class CaptureEngine: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate, AVCaptureAudioDataOutputSampleBufferDelegate {
    let session = AVCaptureSession()
    let encoder = VideoEncoder()
    let audio = AudioPacketizer()
    let queue = DispatchQueue(label: "titancam.capture", qos: .userInitiated)
    let audioQueue = DispatchQueue(label: "titancam.audio", qos: .userInteractive)
    var output: ((EncodedUnit) -> Void)?
    var failure: ((String) -> Void)?
    private(set) var epoch: UInt64 = 0
    private(set) var effective = StreamConfig()
    private var cameraInput: AVCaptureDeviceInput?
    private var configured = false
    private var captureSessionID = Data()
    private var observers: [NSObjectProtocol] = []
    private let timeline = NSLock()
    private var ignoreRouteUntil: UInt64 = 0
    static var hostTime: UInt64 {
        let time = CMTimeConvertScale(CMClockGetTime(CMClockGetHostTimeClock()), timescale: 1_000_000_000, method: .default)
        return UInt64(max(0, time.value))
    }
    override init() {
        super.init()
        encoder.output = { [weak self] in self?.output?($0) }
        audio.output = { [weak self] in self?.output?($0) }
        audio.failure = { [weak self] in self?.failure?($0) }
        encoder.failure = { [weak self] in self?.failure?($0) }
        for name in [AVCaptureSession.wasInterruptedNotification, AVCaptureSession.runtimeErrorNotification, AVAudioSession.interruptionNotification, AVAudioSession.routeChangeNotification] {
            observers.append(NotificationCenter.default.addObserver(forName: name, object: nil, queue: nil) { [weak self] notification in
                self?.queue.async {
                    guard let self, self.configured else { return }
                    if name == AVAudioSession.routeChangeNotification && (!self.session.isRunning || Self.hostTime < self.ignoreRouteUntil) { return }
                    if name == AVAudioSession.interruptionNotification,
                       (notification.userInfo?[AVAudioSessionInterruptionTypeKey] as? NSNumber)?.uintValue != AVAudioSession.InterruptionType.began.rawValue { return }
                    self.failure?("Capture interrupted: \(name.rawValue). Return to TitanCam or reconnect after the audio route is ready.")
                }
            })
        }
    }
    static func authorize() async -> Bool {
        let camera = await AVCaptureDevice.requestAccess(for: .video)
        let microphone = await AVCaptureDevice.requestAccess(for: .audio)
        return camera && microphone
    }
    func configure(_ requested: StreamConfig, sessionID: Data, completion: @escaping (Result<StreamConfig, Error>) -> Void) {
        queue.async {
            let previous = self.effective
            let previousSession = self.captureSessionID
            let wasConfigured = self.configured
            let running = self.session.isRunning
            do {
                let applied = try self.apply(requested, sessionID: sessionID)
                completion(.success(applied))
            } catch {
                if !wasConfigured {
                    self.session.beginConfiguration()
                    for input in self.session.inputs { self.session.removeInput(input) }
                    for output in self.session.outputs { self.session.removeOutput(output) }
                    self.session.commitConfiguration(); self.cameraInput = nil; self.configured = false
                }
                if wasConfigured {
                    do {
                        _ = try self.apply(previous, sessionID: previousSession)
                        if running { self.encoder.requestIDR(); self.session.startRunning() }
                    } catch { self.failure?("Capture rollback failed: \(error.localizedDescription)") }
                }
                completion(.failure(error))
            }
        }
    }
    private func apply(_ requested: StreamConfig, sessionID: Data) throws -> StreamConfig {
        guard requested.valid else { throw CameraError.protocolViolation("Unsupported configuration") }
        guard let device = CameraCatalog.camera(requested.camera_id) else { throw CameraError.unavailable("Selected camera is unavailable") }
        var config = requested
        let match: (Int, Int, Int) -> AVCaptureDevice.Format? = { width, height, fps in
            device.formats.first { format in
                let dimensions = CMVideoFormatDescriptionGetDimensions(format.formatDescription)
                return Int(dimensions.width) == width && Int(dimensions.height) == height && format.videoSupportedFrameRateRanges.contains { $0.minFrameRate <= Double(fps) && $0.maxFrameRate >= Double(fps) }
            }
        }
        var format = match(config.width, config.height, config.fps)
        if format == nil {
            config.width = 1920; config.height = 1080; config.fps = min(30, config.fps)
            config.bitrate = min(config.bitrate, 14_000_000)
            config.fallback_reason = [config.fallback_reason, "Requested format unavailable on selected camera; using 1080p at up to 30 FPS"].compactMap { $0 }.joined(separator: "; ")
            format = match(config.width, config.height, config.fps)
        }
        guard let format else { throw CameraError.unavailable("No compatible camera format") }
        let replacement = cameraInput?.device.uniqueID == device.uniqueID ? nil : try AVCaptureDeviceInput(device: device)
        if session.isRunning { session.stopRunning() }
        encoder.stop()
        session.beginConfiguration()
        do {
            session.automaticallyConfiguresApplicationAudioSession = false
            if let replacement {
                let old = cameraInput
                if let old { session.removeInput(old) }
                guard session.canAddInput(replacement) else {
                    if let old, session.canAddInput(old) { session.addInput(old) }
                    throw CameraError.unavailable("Camera input cannot be added")
                }
                session.addInput(replacement); cameraInput = replacement
            }
            if !configured {
                guard let microphone = AVCaptureDevice.default(for: .audio) else { throw CameraError.unavailable("Microphone unavailable") }
                let mic = try AVCaptureDeviceInput(device: microphone)
                guard session.canAddInput(mic) else { throw CameraError.unavailable("Microphone input unavailable") }
                session.addInput(mic)
                let video = AVCaptureVideoDataOutput()
                video.alwaysDiscardsLateVideoFrames = true
                video.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String:kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange]
                video.setSampleBufferDelegate(self, queue: encoder.queue)
                guard session.canAddOutput(video) else { throw CameraError.unavailable("Video output unavailable") }
                session.addOutput(video)
                let output = AVCaptureAudioDataOutput()
                output.setSampleBufferDelegate(self, queue: audioQueue)
                guard session.canAddOutput(output) else { throw CameraError.unavailable("Audio output unavailable") }
                session.addOutput(output)
                configured = true
            }
            if let video = session.outputs.first(where: { $0 is AVCaptureVideoDataOutput }), let connection = video.connection(with: .video) {
                connection.videoOrientation = .landscapeRight
                if connection.isVideoMirroringSupported { connection.automaticallyAdjustsVideoMirroring = false; connection.isVideoMirrored = false }
                if connection.isVideoStabilizationSupported { connection.preferredVideoStabilizationMode = .off }
            }
            try device.lockForConfiguration()
            device.activeFormat = format
            device.activeVideoMinFrameDuration = CMTime(value: 1, timescale: CMTimeScale(config.fps))
            device.activeVideoMaxFrameDuration = CMTime(value: 1, timescale: CMTimeScale(config.fps))
            VideoHDRPolicy.applySDR(to: device)
            device.unlockForConfiguration()
            session.commitConfiguration()
        } catch { session.commitConfiguration(); throw error }
        let audioSession = AVAudioSession.sharedInstance()
        ignoreRouteUntil = Self.hostTime + 1_000_000_000
        try audioSession.setCategory(.record, mode: .measurement)
        try audioSession.setPreferredSampleRate(48_000)
        try audioSession.setPreferredIOBufferDuration(0.005)
        try audioSession.setActive(true)
        if let inputID = config.audio_input_id {
            guard let port = audioSession.availableInputs?.first(where: { $0.uid == inputID }) else { throw CameraError.unavailable("Selected microphone input is unavailable") }
            if let sourceID = config.audio_data_source {
                guard let source = port.dataSources?.first(where: { $0.dataSourceID.uint32Value == sourceID }) else { throw CameraError.unavailable("Selected microphone data source is unavailable") }
                try port.setPreferredDataSource(source)
            } else {
                try port.setPreferredDataSource(nil)
            }
            try audioSession.setPreferredInput(port)
        } else {
            try audioSession.setPreferredInput(nil)
        }
        if let port = audioSession.currentRoute.inputs.first {
            let actualSource = port.selectedDataSource?.dataSourceID.uint32Value
            if let requestedInput = config.audio_input_id, requestedInput != port.uid {
                config.fallback_reason = [config.fallback_reason, "iOS selected a different microphone input; effective route reported"].compactMap { $0 }.joined(separator: "; ")
            } else if let requestedSource = config.audio_data_source, requestedSource != actualSource {
                config.fallback_reason = [config.fallback_reason, "iOS selected a different microphone data source; effective source reported"].compactMap { $0 }.joined(separator: "; ")
            }
            config.audio_input_id = port.uid; config.audio_data_source = actualSource
        }
        config.audio_channels = 1
        config.camera_id = device.uniqueID
        try encoder.queue.sync { try encoder.configure(config, session: sessionID) }
        try audioQueue.sync { try audio.configure(config, session: sessionID) }
        config.encoder_hardware_evidence = encoder.hardwareEvidence
        config.encoder_hardware_query_status = Int(encoder.hardwareQueryStatus)
        timeline.lock()
        if captureSessionID != sessionID { epoch = Self.hostTime; captureSessionID = sessionID }
        timeline.unlock()
        effective = config
        return config
    }
    func capabilities() -> [String: Any] { queue.sync { CameraCatalog.capabilities() } }
    func adjustBitrate(_ bitrate: Int, completion: @escaping (Result<StreamConfig, Error>) -> Void) {
        queue.async {
            do { try self.encoder.queue.sync { try self.encoder.setBitrate(bitrate) }; self.effective.bitrate = bitrate; completion(.success(self.effective)) }
            catch { completion(.failure(error)) }
        }
    }
    func start() { queue.async { self.session.startRunning() } }
    func stop() {
        queue.async {
            if self.session.isRunning { self.session.stopRunning() }
            self.encoder.stop()
            try? AVAudioSession.sharedInstance().setActive(false)
        }
    }
    func requestIDR() { encoder.requestIDR() }
    func videoStatistics(completion: @escaping ([String: Any]) -> Void) { encoder.queue.async { completion(self.encoder.statistics()) } }
    func captureOutput(_ output: AVCaptureOutput, didDrop sample: CMSampleBuffer, from connection: AVCaptureConnection) { if output is AVCaptureVideoDataOutput { encoder.captureDropped() } }
    func captureOutput(_ output: AVCaptureOutput, didOutput sample: CMSampleBuffer, from connection: AVCaptureConnection) {
        let pts = CMSampleBufferGetPresentationTimeStamp(sample)
        let host = session.masterClock.map { CMSyncConvertTime(pts, from: $0, to: CMClockGetHostTimeClock()) } ?? pts
        let absolute = UInt64(max(0, CMTimeConvertScale(host, timescale: 1_000_000_000, method: .default).value))
        timeline.lock(); let origin = epoch; timeline.unlock()
        guard absolute >= origin else { return }
        if output is AVCaptureVideoDataOutput { encoder.encode(sample, pts: absolute - origin) }
        else { audio.consume(sample, pts: absolute - origin) }
    }
    deinit { for observer in observers { NotificationCenter.default.removeObserver(observer) } }
}
