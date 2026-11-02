package shop.repo;

import java.util.List;
import org.springframework.data.jpa.repository.JpaRepository;
import org.springframework.data.jpa.repository.Query;
import shop.model.User;

public interface UserRepository extends JpaRepository<User, Long> {
    @Query("select u from User u left join fetch Permission p where p.user = u")
    List<User> findAllWithPermissions();

    @Query("select distinct u from User u join fetch u.roles where u.active = true")
    List<User> findActiveWithRoles();
}
