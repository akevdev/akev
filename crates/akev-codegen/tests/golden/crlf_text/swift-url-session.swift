import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

let body = """
    --XyZ\r
    Content-Disposition: form-data; name="note"\r
    \r
    hi \u{1B}[1m"there"\r
    --XyZ--\r

    """

var request = URLRequest(url: URL(string: "https://example.com/upload")!)
request.httpMethod = "POST"
request.setValue("multipart/form-data; boundary=XyZ", forHTTPHeaderField: "Content-Type")
request.httpBody = Data(body.utf8)

let (data, response) = try await URLSession.shared.data(for: request)

print((response as? HTTPURLResponse)?.statusCode ?? 0)
print(String(decoding: data, as: UTF8.self))