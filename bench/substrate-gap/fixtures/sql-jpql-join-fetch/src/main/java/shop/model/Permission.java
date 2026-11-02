package shop.model;

import javax.persistence.*;

@Entity
public class Permission {
    @Id
    private Long id;

    @ManyToOne
    private User user;
}
