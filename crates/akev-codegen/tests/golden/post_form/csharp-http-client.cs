using System;
using System.Net.Http;
using System.Net.Http.Headers;

using var client = new HttpClient();
using var request = new HttpRequestMessage(HttpMethod.Post, "https://example.com/login");
request.Headers.TryAddWithoutValidation("Cookie", "session=abc123; theme=dark");
request.Headers.TryAddWithoutValidation("Origin", "https://example.com");
var content = new StringContent("user=ann%40example.com&password=p%40ss+w0rd%21&remember=on");
content.Headers.ContentType = MediaTypeHeaderValue.Parse("application/x-www-form-urlencoded");
request.Content = content;

using var response = await client.SendAsync(request);

Console.WriteLine((int)response.StatusCode);
Console.WriteLine(await response.Content.ReadAsStringAsync());