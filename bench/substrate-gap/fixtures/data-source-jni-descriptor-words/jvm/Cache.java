import org.redisson.Redisson;
import org.redisson.api.RedissonClient;

public class Cache {
    RedissonClient client() {
        return Redisson.create();
    }
}
