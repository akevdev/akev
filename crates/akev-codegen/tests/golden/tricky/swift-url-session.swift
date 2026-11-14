import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

let body = """
    Line 1: it's "quoted" and 'single'
    Line 2: $HOME ${HOME} `whoami` $(id) %PATH% 100% #{x} #@y !!
    	Tabbed: back\\slash \\n \\\\ "\"\" ''' ``` --%
    Unicode: héllo — 日本語 🚀 ‘smart’ “quotes”
    '@ at line start, and a trailing backslash \\
    """

var request = URLRequest(url: URL(string: "https://example.com/api/notes/1?q=a%20b&x=$y&z='%22")!)
request.httpMethod = "PATCH"
request.setValue("text/plain; charset=utf-8", forHTTPHeaderField: "Content-Type")
request.setValue("it's \"quoted\" `cmd` $HOME %PATH% \\back\\", forHTTPHeaderField: "X-Quote")
request.setValue("héllo 日本 ✓", forHTTPHeaderField: "X-Unicode")
request.setValue("=starts-with-equals", forHTTPHeaderField: "X-Eq")
request.httpBody = Data(body.utf8)

let (data, response) = try await URLSession.shared.data(for: request)

print((response as? HTTPURLResponse)?.statusCode ?? 0)
print(String(decoding: data, as: UTF8.self))