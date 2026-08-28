import AVFoundation
import SwiftUI
import UIKit

struct QRCodeScannerView: UIViewControllerRepresentable {
    let onCode: (String) -> Void
    let onFailure: (String) -> Void

    func makeUIViewController(context: Context) -> QRCodeScannerViewController {
        QRCodeScannerViewController(onCode: onCode, onFailure: onFailure)
    }

    func updateUIViewController(_ uiViewController: QRCodeScannerViewController, context: Context) {}
}

final class QRCodeScannerViewController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    private let captureSession = AVCaptureSession()
    private let sessionQueue = DispatchQueue(label: "com.beale.ios.qr-scanner")
    private let onCode: (String) -> Void
    private let onFailure: (String) -> Void
    private var previewLayer: AVCaptureVideoPreviewLayer?
    private var configured = false
    private var completed = false

    init(onCode: @escaping (String) -> Void, onFailure: @escaping (String) -> Void) {
        self.onCode = onCode
        self.onFailure = onFailure
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        requestCameraAccess()
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        previewLayer?.frame = view.bounds
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        sessionQueue.async { [captureSession] in
            if captureSession.isRunning {
                captureSession.stopRunning()
            }
        }
    }

    private func requestCameraAccess() {
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized:
            configureCamera()
        case .notDetermined:
            AVCaptureDevice.requestAccess(for: .video) { [weak self] granted in
                DispatchQueue.main.async {
                    guard let self else { return }
                    if granted {
                        self.configureCamera()
                    } else {
                        self.fail("Camera access is required to scan the app-server QR code.")
                    }
                }
            }
        case .denied, .restricted:
            fail("Camera access is required to scan the app-server QR code. Enable it in iOS Settings.")
        @unknown default:
            fail("The camera is unavailable.")
        }
    }

    private func configureCamera() {
        guard !configured else { return }
        configured = true
        guard let camera = AVCaptureDevice.default(for: .video) else {
            fail("No camera is available on this device.")
            return
        }

        do {
            let input = try AVCaptureDeviceInput(device: camera)
            guard captureSession.canAddInput(input) else {
                fail("The camera could not be configured for QR scanning.")
                return
            }
            captureSession.addInput(input)

            let output = AVCaptureMetadataOutput()
            guard captureSession.canAddOutput(output) else {
                fail("QR scanning is unavailable on this device.")
                return
            }
            captureSession.addOutput(output)
            output.setMetadataObjectsDelegate(self, queue: .main)
            output.metadataObjectTypes = [.qr]

            let preview = AVCaptureVideoPreviewLayer(session: captureSession)
            preview.videoGravity = .resizeAspectFill
            preview.frame = view.bounds
            view.layer.addSublayer(preview)
            previewLayer = preview

            sessionQueue.async { [captureSession] in
                captureSession.startRunning()
            }
        } catch {
            fail("The camera could not be opened: \(error.localizedDescription)")
        }
    }

    func metadataOutput(
        _ output: AVCaptureMetadataOutput,
        didOutput metadataObjects: [AVMetadataObject],
        from connection: AVCaptureConnection
    ) {
        guard !completed,
              let code = metadataObjects
                .compactMap({ $0 as? AVMetadataMachineReadableCodeObject })
                .first(where: { $0.type == .qr })?.stringValue else {
            return
        }
        completed = true
        sessionQueue.async { [captureSession] in
            if captureSession.isRunning {
                captureSession.stopRunning()
            }
        }
        onCode(code)
    }

    private func fail(_ message: String) {
        guard !completed else { return }
        completed = true
        onFailure(message)
    }
}
