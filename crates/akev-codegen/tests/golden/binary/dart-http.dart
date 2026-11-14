import 'dart:convert';

import 'package:http/http.dart' as http;

Future<void> main() async {
  final request = http.Request('PUT', Uri.parse('https://upload.example.com/files/logo.png'));
  request.headers.addAll({
    'Content-Type': 'image/png',
    'X-Upload-Token': 't0k3n',
  });
  request.bodyBytes = base64Decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ//4iJ1w=');

  final response = await http.Response.fromStream(await request.send());
  print(response.statusCode);
  print(response.body);
}