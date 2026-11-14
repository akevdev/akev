import 'dart:convert';

import 'package:http/http.dart' as http;

// Note: dart:io rejects non-ASCII header values (FormatException); the X-Unicode header value needs to be ASCII for this to work as captured.
Future<void> main() async {
  final request = http.Request('PATCH', Uri.parse("https://example.com/api/notes/1?q=a%20b&x=\$y&z='%22"));
  request.headers.addAll({
    'Content-Type': 'text/plain; charset=utf-8',
    'X-Quote': 'it\'s "quoted" `cmd` \$HOME %PATH% \\back\\',
    'X-Unicode': 'héllo 日本 ✓',
    'X-Eq': '=starts-with-equals',
  });
  request.bodyBytes = utf8.encode("Line 1: it's \"quoted\" and 'single'\nLine 2: \$HOME \${HOME} `whoami` \$(id) %PATH% 100% #{x} #@y !!\n\tTabbed: back\\slash \\n \\\\ \"\"\" ''' ``` --%\nUnicode: héllo — 日本語 🚀 ‘smart’ “quotes”\n'@ at line start, and a trailing backslash \\");

  final response = await http.Response.fromStream(await request.send());
  print(response.statusCode);
  print(response.body);
}