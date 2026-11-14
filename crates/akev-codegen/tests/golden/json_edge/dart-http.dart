import 'dart:convert';

import 'package:http/http.dart' as http;

Future<void> main() async {
  final request = http.Request('POST', Uri.parse('https://example.com/graphql'));
  request.headers.addAll({
    'Content-Type': 'application/json; charset=utf-8',
  });
  request.bodyBytes = utf8.encode(r'''
{
  "__proto__": {"polluted": true},
  "0": "zero",
  "big": 9007199254740993,
  "text": "line\nbreak \"q\" $x #{y} \u00e9",
  "empty": [],
  "nested": [[1, 2], {"a": null}],
  "neg": -1.5e-7
}''');

  final response = await http.Response.fromStream(await request.send());
  print(response.statusCode);
  print(response.body);
}