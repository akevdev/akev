import 'dart:convert';

import 'package:http/http.dart' as http;

Future<void> main() async {
  final request = http.Request('POST', Uri.parse('https://api.example.com/v1/users'));
  request.headers.addAll({
    'Content-Type': 'application/json',
    'Accept': 'application/json',
  });
  request.bodyBytes = utf8.encode('{"name":"Ann O\'Neil","age":34,"admin":false,"tags":["a","b"],"address":{"city":"Zürich","zip":null},"score":9.5,"meta":{}}');

  final response = await http.Response.fromStream(await request.send());
  print(response.statusCode);
  print(response.body);
}