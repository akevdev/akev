import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody

fun main() {
    val client = OkHttpClient()

    val body = """
        |{
        |  "__proto__": {"polluted": true},
        |  "0": "zero",
        |  "big": 9007199254740993,
        |  "text": "line\nbreak \"q\" ${'$'}x #{y} \u00e9",
        |  "empty": [],
        |  "nested": [[1, 2], {"a": null}],
        |  "neg": -1.5e-7
        |}
        """.trimMargin().toRequestBody("application/json; charset=utf-8".toMediaType())

    val request = Request.Builder()
        .url("https://example.com/graphql")
        .post(body)
        .build()

    client.newCall(request).execute().use { response ->
        println(response.code)
        println(response.body?.string())
    }
}