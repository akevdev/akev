using System;
using System.Net;
using System.Net.Http;

using var client = new HttpClient(new SocketsHttpHandler
{
    AutomaticDecompression = DecompressionMethods.All,
});
using var request = new HttpRequestMessage(HttpMethod.Get, "https://api.example.com/v1/items?page=2&sort=-created");
request.Headers.TryAddWithoutValidation("Accept", "application/json");
request.Headers.TryAddWithoutValidation("Authorization", "Bearer eyJhbGciOiJIUzI1NiJ9.e30.abc");
request.Headers.TryAddWithoutValidation("User-Agent", "Akev/1.0");
request.Headers.TryAddWithoutValidation("X-Request-Id", "42");

using var response = await client.SendAsync(request);

Console.WriteLine((int)response.StatusCode);
Console.WriteLine(await response.Content.ReadAsStringAsync());