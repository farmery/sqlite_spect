# Podspec for the sqlite_spect Flutter FFI plugin on iOS.
#
# The Rust engine ships as an xcframework built ahead of the pod install (see
# `scripts/build_ios.sh`). Consumers who use the recommended
# `dev_dependencies:` pattern won't ship this in their release builds.

Pod::Spec.new do |s|
  s.name             = 'sqlite_spect'
  s.version          = '0.1.0'
  s.summary          = 'In-app SQLite inspector for Flutter (debug/staging only).'
  s.description      = <<-DESC
Embeds the sqlite_spect Rust engine as a static xcframework so a Flutter
app can call Inspector.start(...) and expose its SQLite database(s) over a
loopback HTTP+WebSocket server to a browser-based inspector.
                       DESC
  s.homepage         = 'https://github.com/farmery/sqlite_spect'
  s.license          = { :file => '../LICENSE' }
  s.author           = { 'farmery' => 'nwachiifeanyi3000@gmail.com' }
  s.source           = { :path => '.' }
  s.source_files     = 'Classes/**/*'
  s.dependency 'Flutter'
  s.platform         = :ios, '12.0'

  # The vendored framework is built by scripts/build_ios.sh — must exist for
  # pod install to succeed.
  s.vendored_frameworks = 'Frameworks/sqlite_spect_ffi.xcframework'

  s.pod_target_xcconfig = { 'DEFINES_MODULE' => 'YES' }
  s.swift_version = '5.0'
end
