using System;
using System.Net.Http;

using var client = new HttpClient();
using var request = new HttpRequestMessage(new HttpMethod("purge"), "http://cache.local:8080/a/../b/[1]?x={y}");
request.Headers.TryAddWithoutValidation("Host", "example.com");
request.Headers.TryAddWithoutValidation("X-Dup", "one");
request.Headers.TryAddWithoutValidation("x-dup", "two");
request.Headers.TryAddWithoutValidation("X-Empty", "");
var content = new StringContent("plain body without a content type");
content.Headers.ContentType = null;
request.Content = content;

using var response = await client.SendAsync(request);

Console.WriteLine((int)response.StatusCode);
Console.WriteLine(await response.Content.ReadAsStringAsync());