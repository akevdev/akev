import 'dart:convert';

import 'package:http/http.dart' as http;

Future<void> main() async {
  final request = http.Request('purge', Uri.parse('http://cache.local:8080/a/../b/%5B1%5D?x=%7By%7D'));
  request.headers.addAll({
    'Host': 'example.com',
    'X-Dup': 'one, two',
    'X-Empty': '',
  });
  request.bodyBytes = utf8.encode('plain body without a content type');

  final response = await http.Response.fromStream(await request.send());
  print(response.statusCode);
  print(response.body);
}