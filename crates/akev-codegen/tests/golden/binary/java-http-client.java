import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.util.Base64;

public class Main {
    public static void main(String[] args) throws Exception {
        byte[] body = Base64.getDecoder().decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ//4iJ1w=");

        HttpRequest request = HttpRequest.newBuilder()
                .uri(URI.create("https://upload.example.com/files/logo.png"))
                .header("Content-Type", "image/png")
                .header("X-Upload-Token", "t0k3n")
                .PUT(HttpRequest.BodyPublishers.ofByteArray(body))
                .build();

        HttpResponse<String> response = HttpClient.newHttpClient()
                .send(request, HttpResponse.BodyHandlers.ofString());

        System.out.println(response.statusCode());
        System.out.println(response.body());
    }
}