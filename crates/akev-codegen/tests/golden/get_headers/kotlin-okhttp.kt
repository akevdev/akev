import okhttp3.OkHttpClient
import okhttp3.Request

fun main() {
    val client = OkHttpClient()

    val request = Request.Builder()
        .url("https://api.example.com/v1/items?page=2&sort=-created")
        .addHeader("Accept", "application/json")
        .addHeader("Authorization", "Bearer eyJhbGciOiJIUzI1NiJ9.e30.abc")
        .addHeader("User-Agent", "Akev/1.0")
        .addHeader("X-Request-Id", "42")
        .build()

    client.newCall(request).execute().use { response ->
        println(response.code)
        println(response.body?.string())
    }
}