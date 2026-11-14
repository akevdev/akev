import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;

public class Main {
    public static void main(String[] args) throws Exception {
        String body = """
                Line 1: it's "quoted" and 'single'
                Line 2: $HOME ${HOME} `whoami` $(id) %PATH% 100% #{x} #@y !!
                	Tabbed: back\\slash \\n \\\\ "\"\" ''' ``` --%
                Unicode: héllo — 日本語 🚀 ‘smart’ “quotes”
                '@ at line start, and a trailing backslash \\\
                """;

        // Note: java.net.http sends non-ASCII header characters as '?' (and rejects those above U+00FF); the X-Unicode header value needs to be ASCII for this to work as captured.
        HttpRequest request = HttpRequest.newBuilder()
                .uri(URI.create("https://example.com/api/notes/1?q=a%20b&x=$y&z='%22"))
                .header("Content-Type", "text/plain; charset=utf-8")
                .header("X-Quote", "it's \"quoted\" `cmd` $HOME %PATH% \\back\\")
                .header("X-Unicode", "héllo 日本 ✓")
                .header("X-Eq", "=starts-with-equals")
                .method("PATCH", HttpRequest.BodyPublishers.ofString(body))
                .build();

        HttpResponse<String> response = HttpClient.newHttpClient()
                .send(request, HttpResponse.BodyHandlers.ofString());

        System.out.println(response.statusCode());
        System.out.println(response.body());
    }
}