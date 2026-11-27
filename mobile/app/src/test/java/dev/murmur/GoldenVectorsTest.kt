package dev.murmur

import dev.murmur.core.Msg
import dev.murmur.core.protoJson
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/**
 * Golden wire vectors shared with the Rust side — the same
 * shared/proto/tests/vectors.json is asserted byte-for-byte in the
 * Rust test suite. If the two implementations diverge in wire format,
 * one of the two suites goes red.
 */
class GoldenVectorsTest {
    private val vectorsFile = File("../..", "shared/proto/tests/vectors.json")

    @Test
    fun everyVectorParsesAsAMessage() {
        assumeFilePresent()
        val arr = Json.parseToJsonElement(vectorsFile.readText()).jsonArray
        assertTrue("vectors file is empty", arr.size > 5)
        for (v in arr) {
            val decoded = protoJson.decodeFromJsonElement(Msg.serializer(), v)
            // the serde "t" tag survives the roundtrip — the dispatch
            // contract both implementations rely on
            val re = protoJson.encodeToJsonElement(Msg.serializer(), decoded)
            assertEquals(v.jsonObject["t"], re.jsonObject["t"])
        }
    }

    private fun assumeFilePresent() {
        assertTrue(
            "vectors.json not found at ${vectorsFile.absolutePath} — test must run from the mobile/ module dir",
            vectorsFile.exists()
        )
    }
}
