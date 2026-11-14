import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;

public class Main {
    public static void main(String[] args) throws Exception {
        String body = """
                {
                  "__proto__": {"polluted": true},
                  "0": "zero",
                  "big": 9007199254740993,
                  "text": "line\\nbreak \\"q\\" $x #{y} \\u00e9",
                  "empty": [],
                  "nested": [[1, 2], {"a": null}],
                  "neg": -1.5e-7
                }\
                """;

        HttpRequest request = HttpRequest.newBuilder()
                .uri(URI.create("https://example.com/graphql"))
                .header("Content-Type", "application/json; charset=utf-8")
                .POST(HttpRequest.BodyPublishers.ofString(body))
                .build();

        HttpResponse<String> response = HttpClient.newHttpClient()
                .send(request, HttpResponse.BodyHandlers.ofString());

        System.out.println(response.statusCode());
        System.out.println(response.body());
    }
}