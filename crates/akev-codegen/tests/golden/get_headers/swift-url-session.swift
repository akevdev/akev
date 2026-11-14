import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

var request = URLRequest(url: URL(string: "https://api.example.com/v1/items?page=2&sort=-created")!)
request.setValue("application/json", forHTTPHeaderField: "Accept")
request.setValue("Bearer eyJhbGciOiJIUzI1NiJ9.e30.abc", forHTTPHeaderField: "Authorization")
request.setValue("Akev/1.0", forHTTPHeaderField: "User-Agent")
request.setValue("42", forHTTPHeaderField: "X-Request-Id")

let (data, response) = try await URLSession.shared.data(for: request)

print((response as? HTTPURLResponse)?.statusCode ?? 0)
print(String(decoding: data, as: UTF8.self))