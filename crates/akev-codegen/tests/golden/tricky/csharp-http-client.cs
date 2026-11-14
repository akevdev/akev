using System;
using System.Net.Http;
using System.Net.Http.Headers;
using System.Text;

using var client = new HttpClient(new SocketsHttpHandler
{
    // Send non-ASCII header values as UTF-8 (by default .NET rejects them).
    RequestHeaderEncodingSelector = (_, _) => Encoding.UTF8,
});
using var request = new HttpRequestMessage(HttpMethod.Patch, "https://example.com/api/notes/1?q=a%20b&x=$y&z='\"");
request.Headers.TryAddWithoutValidation("X-Quote", "it's \"quoted\" `cmd` $HOME %PATH% \\back\\");
request.Headers.TryAddWithoutValidation("X-Unicode", "héllo 日本 ✓");
request.Headers.TryAddWithoutValidation("X-Eq", "=starts-with-equals");
var content = new StringContent(""""
    Line 1: it's "quoted" and 'single'
    Line 2: $HOME ${HOME} `whoami` $(id) %PATH% 100% #{x} #@y !!
    	Tabbed: back\slash \n \\ """ ''' ``` --%
    Unicode: héllo — 日本語 🚀 ‘smart’ “quotes”
    '@ at line start, and a trailing backslash \
    """");
content.Headers.ContentType = MediaTypeHeaderValue.Parse("text/plain; charset=utf-8");
request.Content = content;

using var response = await client.SendAsync(request);

Console.WriteLine((int)response.StatusCode);
Console.WriteLine(await response.Content.ReadAsStringAsync());