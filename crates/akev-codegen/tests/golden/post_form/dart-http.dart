import 'dart:convert';

import 'package:http/http.dart' as http;

Future<void> main() async {
  final request = http.Request('POST', Uri.parse('https://example.com/login'));
  request.headers.addAll({
    'Content-Type': 'application/x-www-form-urlencoded',
    'Cookie': 'session=abc123; theme=dark',
    'Origin': 'https://example.com',
  });
  request.bodyBytes = utf8.encode('user=ann%40example.com&password=p%40ss+w0rd%21&remember=on');

  final response = await http.Response.fromStream(await request.send());
  print(response.statusCode);
  print(response.body);
}