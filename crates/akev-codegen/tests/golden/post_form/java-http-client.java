import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;

public class Main {
    public static void main(String[] args) throws Exception {
        String body = "user=ann%40example.com&password=p%40ss+w0rd%21&remember=on";

        HttpRequest request = HttpRequest.newBuilder()
                .uri(URI.create("https://example.com/login"))
                .header("Content-Type", "application/x-www-form-urlencoded")
                .header("Cookie", "session=abc123; theme=dark")
                .header("Origin", "https://example.com")
                .POST(HttpRequest.BodyPublishers.ofString(body))
                .build();

        HttpResponse<String> response = HttpClient.newHttpClient()
                .send(request, HttpResponse.BodyHandlers.ofString());

        System.out.println(response.statusCode());
        System.out.println(response.body());
    }
}