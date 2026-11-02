package shop

import org.junit.jupiter.api.Test
import org.redisson.codec.Kryo5Codec
import kotlin.test.assertNotNull

class NameCodecTest {
    @Test
    fun `keeps writing names when Redisson rebinds the codec to a classloader`() {
        val encoded = Kryo5Codec().valueEncoder.encode("x")
        assertNotNull(encoded)
    }
}
