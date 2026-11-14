import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;

public class Main {
    public static void main(String[] args) throws Exception {
        String body = """
                --XyZ\r
                Content-Disposition: form-data; name="note"\r
                \r
                hi \033[1m"there"\r
                --XyZ--\r
                """;

        HttpRequest request = HttpRequest.newBuilder()
                .uri(URI.create("https://example.com/upload"))
                .header("Content-Type", "multipart/form-data; boundary=XyZ")
                .POST(HttpRequest.BodyPublishers.ofString(body))
                .build();

        HttpResponse<String> response = HttpClient.newHttpClient()
                .send(request, HttpResponse.BodyHandlers.ofString());

        System.out.println(response.statusCode());
        System.out.println(response.body());
    }
}