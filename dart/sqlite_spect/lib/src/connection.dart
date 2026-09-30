/// Connection details returned from [Inspector.start].
class InspectorConnection {
  final String host;
  final int port;

  const InspectorConnection({
    required this.host,
    required this.port,
  });

  /// Loopback URL for the inspector server.
  String get url => 'http://$host:$port';
}
