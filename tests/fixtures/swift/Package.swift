// swift-tools-version:5.9
import PackageDescription

let package = Package(name: "Shop", targets: [.target(name: "Domain"), .target(name: "Infra", dependencies: ["Domain"]), .target(name: "Web", dependencies: ["Domain", "Infra"])])
