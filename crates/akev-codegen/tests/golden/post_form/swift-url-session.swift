import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

let body = "user=ann%40example.com&password=p%40ss+w0rd%21&remember=on"

var request = URLRequest(url: URL(string: "https://example.com/login")!)
request.httpMethod = "POST"
request.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")
request.setValue("session=abc123; theme=dark", forHTTPHeaderField: "Cookie")
request.setValue("https://example.com", forHTTPHeaderField: "Origin")
request.httpBody = Data(body.utf8)

let (data, response) = try await URLSession.shared.data(for: request)

print((response as? HTTPURLResponse)?.statusCode ?? 0)
print(String(decoding: data, as: UTF8.self))