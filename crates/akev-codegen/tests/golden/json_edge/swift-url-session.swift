import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

let body = """
    {
      "__proto__": {"polluted": true},
      "0": "zero",
      "big": 9007199254740993,
      "text": "line\\nbreak \\"q\\" $x #{y} \\u00e9",
      "empty": [],
      "nested": [[1, 2], {"a": null}],
      "neg": -1.5e-7
    }
    """

var request = URLRequest(url: URL(string: "https://example.com/graphql")!)
request.httpMethod = "POST"
request.setValue("application/json; charset=utf-8", forHTTPHeaderField: "Content-Type")
request.httpBody = Data(body.utf8)

let (data, response) = try await URLSession.shared.data(for: request)

print((response as? HTTPURLResponse)?.statusCode ?? 0)
print(String(decoding: data, as: UTF8.self))