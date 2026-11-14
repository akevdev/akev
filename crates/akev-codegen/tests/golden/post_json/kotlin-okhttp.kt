import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody

fun main() {
    val client = OkHttpClient()

    val body = """{"name":"Ann O'Neil","age":34,"admin":false,"tags":["a","b"],"address":{"city":"Zürich","zip":null},"score":9.5,"meta":{}}""".toRequestBody("application/json".toMediaType())

    val request = Request.Builder()
        .url("https://api.example.com/v1/users")
        .post(body)
        .addHeader("Accept", "application/json")
        .build()

    client.newCall(request).execute().use { response ->
        println(response.code)
        println(response.body?.string())
    }
}