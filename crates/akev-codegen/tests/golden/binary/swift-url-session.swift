import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

let body = Data(base64Encoded: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ//4iJ1w=")!

var request = URLRequest(url: URL(string: "https://upload.example.com/files/logo.png")!)
request.httpMethod = "PUT"
request.setValue("image/png", forHTTPHeaderField: "Content-Type")
request.setValue("t0k3n", forHTTPHeaderField: "X-Upload-Token")
request.httpBody = body

let (data, response) = try await URLSession.shared.data(for: request)

print((response as? HTTPURLResponse)?.statusCode ?? 0)
print(String(decoding: data, as: UTF8.self))