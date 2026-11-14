import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;

public class Main {
    public static void main(String[] args) throws Exception {
        HttpRequest request = HttpRequest.newBuilder()
                .uri(URI.create("https://api.example.com/v1/items?page=2&sort=-created"))
                .header("Accept", "application/json")
                .header("Authorization", "Bearer eyJhbGciOiJIUzI1NiJ9.e30.abc")
                .header("User-Agent", "Akev/1.0")
                .header("X-Request-Id", "42")
                .GET()
                .build();

        HttpResponse<String> response = HttpClient.newHttpClient()
                .send(request, HttpResponse.BodyHandlers.ofString());

        System.out.println(response.statusCode());
        System.out.println(response.body());
    }
}