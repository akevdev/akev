import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody

fun main() {
    val client = OkHttpClient()

    val body = "plain body without a content type".toRequestBody()

    val request = Request.Builder()
        .url("http://cache.local:8080/a/../b/[1]?x={y}")
        .method("purge", body)
        .addHeader("Host", "example.com")
        .addHeader("X-Dup", "one")
        .addHeader("x-dup", "two")
        .addHeader("X-Empty", "")
        .build()

    client.newCall(request).execute().use { response ->
        println(response.code)
        println(response.body?.string())
    }
}