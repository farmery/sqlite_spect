// Example app demonstrating sqlite_spect end-to-end:
//
//   * counter.db is opened via sqflite
//   * Inspector.start is called on debug builds only
//   * a debug FAB re-emits the URL banner (spec §12.7) for hot-reload cases
//
// Mobile-only (Android + iOS). Run:
//     flutter run
//     # in another terminal:
//     sqlite_spect attach --watch

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:path/path.dart' as p;
import 'package:path_provider/path_provider.dart';
import 'package:sqflite/sqflite.dart';
import 'package:sqlite_spect/sqlite_spect.dart';

Future<String> _resolveDbPath() async {
  final dir = await getApplicationDocumentsDirectory();
  return p.join(dir.path, 'counter.db');
}

Future<Database> _openCounterDb(String dbPath) async {
  return openDatabase(
    dbPath,
    version: 1,
    onCreate: (db, _) async {
      await db.execute(
        'CREATE TABLE counter (id INTEGER PRIMARY KEY, value INTEGER NOT NULL)',
      );
      await db.execute('INSERT INTO counter (id, value) VALUES (1, 0)');
    },
  );
}

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();

  final dbPath = await _resolveDbPath();
  final db = await _openCounterDb(dbPath);

  // Debug builds only — Inspector.start is a no-op in release, but keeping
  // the call unconditional would still link the native lib. Consumers that
  // want the linker to also drop it should pull the plugin in via
  // dev_dependencies.
  if (kDebugMode) {
    try {
      final conn = await Inspector.start(
        databases: [InspectedDatabase(id: 'counter', path: dbPath)],
        logLevel: 'debug',
      );
      debugPrint('sqlite_spect at ${conn?.url}');
    } on InspectorException catch (e) {
      debugPrint('inspector failed to start: $e');
    }
  }

  runApp(CounterApp(db: db));
}

class CounterApp extends StatelessWidget {
  final Database db;
  const CounterApp({super.key, required this.db});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'sqlite_spect example',
      theme: ThemeData(colorSchemeSeed: Colors.indigo, useMaterial3: true),
      home: CounterHome(db: db),
    );
  }
}

class CounterHome extends StatefulWidget {
  final Database db;
  const CounterHome({super.key, required this.db});

  @override
  State<CounterHome> createState() => _CounterHomeState();
}

class _CounterHomeState extends State<CounterHome> {
  int _value = 0;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    final rows = await widget.db.query('counter', where: 'id = 1');
    setState(() => _value = (rows.first['value'] as int?) ?? 0);
  }

  Future<void> _increment() async {
    final next = _value + 1;
    await widget.db.update('counter', {'value': next}, where: 'id = 1');
    setState(() => _value = next);
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: GestureDetector(
          // Long-press the title to re-emit the URL banner — handy after
          // scrolling past it in `flutter logs`.
          onLongPress: () {
            Inspector.logConnection();
            ScaffoldMessenger.of(context).showSnackBar(
              const SnackBar(
                content: Text('sqlite_spect URL re-logged'),
                duration: Duration(seconds: 2),
              ),
            );
          },
          child: const Text('sqlite_spect example'),
        ),
      ),
      body: Center(
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            const Text('Counter value (persisted to counter.db):'),
            const SizedBox(height: 8),
            Text('$_value', style: Theme.of(context).textTheme.displayLarge),
          ],
        ),
      ),
      floatingActionButton: FloatingActionButton(
        onPressed: _increment,
        tooltip: 'Increment (updates counter.db)',
        child: const Icon(Icons.add),
      ),
    );
  }
}
