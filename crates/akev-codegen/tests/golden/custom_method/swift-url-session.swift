import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

let body = "plain body without a content type"

// Note: URLSession manages the Host header itself and may ignore this value.
var request = URLRequest(url: URL(string: "http://cache.local:8080/a/../b/%5B1%5D?x=%7By%7D")!)
request.httpMethod = "purge"
request.setValue("example.com", forHTTPHeaderField: "Host")
request.setValue("one", forHTTPHeaderField: "X-Dup")
request.addValue("two", forHTTPHeaderField: "x-dup")
request.setValue("", forHTTPHeaderField: "X-Empty")
request.httpBody = Data(body.utf8)

let (data, response) = try await URLSession.shared.data(for: request)

print((response as? HTTPURLResponse)?.statusCode ?? 0)
print(String(decoding: data, as: UTF8.self))