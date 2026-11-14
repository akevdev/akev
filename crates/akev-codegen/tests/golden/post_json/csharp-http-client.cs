using System;
using System.Net.Http;
using System.Net.Http.Headers;

using var client = new HttpClient();
using var request = new HttpRequestMessage(HttpMethod.Post, "https://api.example.com/v1/users");
request.Headers.TryAddWithoutValidation("Accept", "application/json");
var content = new StringContent("""
    {"name":"Ann O'Neil","age":34,"admin":false,"tags":["a","b"],"address":{"city":"Zürich","zip":null},"score":9.5,"meta":{}}
    """);
content.Headers.ContentType = MediaTypeHeaderValue.Parse("application/json");
request.Content = content;

using var response = await client.SendAsync(request);

Console.WriteLine((int)response.StatusCode);
Console.WriteLine(await response.Content.ReadAsStringAsync());