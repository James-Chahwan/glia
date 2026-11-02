package shop.model;

import java.util.Set;
import javax.persistence.*;

@Entity
public class User {
    @Id
    private Long id;
    private boolean active;

    @ElementCollection
    private Set<String> roles;
}
