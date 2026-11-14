import 'package:http/http.dart' as http;

Future<void> main() async {
  final request = http.Request('GET', Uri.parse('https://api.example.com/v1/items?page=2&sort=-created'));
  request.headers.addAll({
    'Accept': 'application/json',
    'Authorization': 'Bearer eyJhbGciOiJIUzI1NiJ9.e30.abc',
    'User-Agent': 'Akev/1.0',
    'X-Request-Id': '42',
  });

  final response = await http.Response.fromStream(await request.send());
  print(response.statusCode);
  print(response.body);
}