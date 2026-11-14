import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;

public class Main {
    public static void main(String[] args) throws Exception {
        // Host is a restricted header in java.net.http unless explicitly allowed.
        System.setProperty("jdk.httpclient.allowRestrictedHeaders", "host");

        String body = "plain body without a content type";

        HttpRequest request = HttpRequest.newBuilder()
                .uri(URI.create("http://cache.local:8080/a/../b/%5B1%5D?x=%7By%7D"))
                .header("Host", "example.com")
                .header("X-Dup", "one")
                .header("x-dup", "two")
                .header("X-Empty", "")
                .method("purge", HttpRequest.BodyPublishers.ofString(body))
                .build();

        HttpResponse<String> response = HttpClient.newHttpClient()
                .send(request, HttpResponse.BodyHandlers.ofString());

        System.out.println(response.statusCode());
        System.out.println(response.body());
    }
}