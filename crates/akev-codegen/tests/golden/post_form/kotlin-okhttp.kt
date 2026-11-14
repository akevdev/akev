import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody

fun main() {
    val client = OkHttpClient()

    val body = "user=ann%40example.com&password=p%40ss+w0rd%21&remember=on".toRequestBody("application/x-www-form-urlencoded".toMediaType())

    val request = Request.Builder()
        .url("https://example.com/login")
        .post(body)
        .addHeader("Cookie", "session=abc123; theme=dark")
        .addHeader("Origin", "https://example.com")
        .build()

    client.newCall(request).execute().use { response ->
        println(response.code)
        println(response.body?.string())
    }
}