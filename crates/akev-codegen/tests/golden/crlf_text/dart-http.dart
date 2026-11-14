import 'dart:convert';

import 'package:http/http.dart' as http;

Future<void> main() async {
  final request = http.Request('POST', Uri.parse('https://example.com/upload'));
  request.headers.addAll({
    'Content-Type': 'multipart/form-data; boundary=XyZ',
  });
  request.bodyBytes = utf8.encode('--XyZ\r\nContent-Disposition: form-data; name="note"\r\n\r\nhi \u{1B}[1m"there"\r\n--XyZ--\r\n');

  final response = await http.Response.fromStream(await request.send());
  print(response.statusCode);
  print(response.body);
}