using System;
using System.Net.Http;
using System.Net.Http.Headers;

using var client = new HttpClient();
using var request = new HttpRequestMessage(HttpMethod.Put, "https://upload.example.com/files/logo.png");
request.Headers.TryAddWithoutValidation("X-Upload-Token", "t0k3n");
var content = new ByteArrayContent(Convert.FromBase64String("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ//4iJ1w="));
content.Headers.ContentType = MediaTypeHeaderValue.Parse("image/png");
request.Content = content;

using var response = await client.SendAsync(request);

Console.WriteLine((int)response.StatusCode);
Console.WriteLine(await response.Content.ReadAsStringAsync());