using System;
using System.Net.Http;
using System.Net.Http.Headers;

using var client = new HttpClient();
using var request = new HttpRequestMessage(HttpMethod.Post, "https://example.com/graphql");
var content = new StringContent("""
    {
      "__proto__": {"polluted": true},
      "0": "zero",
      "big": 9007199254740993,
      "text": "line\nbreak \"q\" $x #{y} \u00e9",
      "empty": [],
      "nested": [[1, 2], {"a": null}],
      "neg": -1.5e-7
    }
    """);
content.Headers.ContentType = MediaTypeHeaderValue.Parse("application/json; charset=utf-8");
request.Content = content;

using var response = await client.SendAsync(request);

Console.WriteLine((int)response.StatusCode);
Console.WriteLine(await response.Content.ReadAsStringAsync());