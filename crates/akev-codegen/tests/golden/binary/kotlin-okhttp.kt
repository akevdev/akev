import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okio.ByteString.Companion.decodeBase64

fun main() {
    val client = OkHttpClient()

    val body = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ//4iJ1w=".decodeBase64()!!.toRequestBody("image/png".toMediaType())

    val request = Request.Builder()
        .url("https://upload.example.com/files/logo.png")
        .put(body)
        .addHeader("X-Upload-Token", "t0k3n")
        .build()

    client.newCall(request).execute().use { response ->
        println(response.code)
        println(response.body?.string())
    }
}