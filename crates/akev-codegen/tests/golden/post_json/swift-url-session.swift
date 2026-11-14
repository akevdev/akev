import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

let body = #"{"name":"Ann O'Neil","age":34,"admin":false,"tags":["a","b"],"address":{"city":"Zürich","zip":null},"score":9.5,"meta":{}}"#

var request = URLRequest(url: URL(string: "https://api.example.com/v1/users")!)
request.httpMethod = "POST"
request.setValue("application/json", forHTTPHeaderField: "Content-Type")
request.setValue("application/json", forHTTPHeaderField: "Accept")
request.httpBody = Data(body.utf8)

let (data, response) = try await URLSession.shared.data(for: request)

print((response as? HTTPURLResponse)?.statusCode ?? 0)
print(String(decoding: data, as: UTF8.self))