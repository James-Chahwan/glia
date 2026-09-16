package db.migration;

import java.sql.Statement;
import org.flywaydb.core.api.migration.BaseJavaMigration;
import org.flywaydb.core.api.migration.Context;

public class V1__Create_users extends BaseJavaMigration {
    @Override
    public void migrate(Context context) throws Exception {
        try (Statement st = context.getConnection().createStatement()) {
            st.execute("CREATE TABLE users (id BIGINT PRIMARY KEY, email VARCHAR(255) NOT NULL)");
        }
    }
}
