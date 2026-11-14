using System;
using System.Net.Http;
using System.Net.Http.Headers;

using var client = new HttpClient();
using var request = new HttpRequestMessage(HttpMethod.Post, "https://example.com/upload");
var content = new StringContent("--XyZ\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nhi \u001B[1m\"there\"\r\n--XyZ--\r\n");
content.Headers.ContentType = MediaTypeHeaderValue.Parse("multipart/form-data; boundary=XyZ");
request.Content = content;

using var response = await client.SendAsync(request);

Console.WriteLine((int)response.StatusCode);
Console.WriteLine(await response.Content.ReadAsStringAsync());