// Probe path: receives recordQuery calls from Dart via FFI.
// Bounded MPSC channel → broadcast to probe.tail subscribers.
