package repo;

import java.util.List;
import org.springframework.data.jpa.repository.Query;
import org.springframework.data.repository.Repository;

// Spring Data JPA: @Query is a JPQL query, not a GraphQL root type.
public interface PetRepository extends Repository<Pet, Integer> {

    @Query("SELECT p FROM Pet p ORDER BY p.name")
    List<Pet> findPets();
}
