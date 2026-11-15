package agentd.conformance;

import static org.junit.jupiter.api.Assertions.assertThrows;

import com.anthropic.errors.UnauthorizedException;
import org.junit.jupiter.api.Test;

/** An admin key reaches only the Admin API, which the Java SDK 2.52.0 does not cover. */
class OrganizationsTest {
    @Test
    void anAdminKeyIsRefusedOnWorkspaceRoutes() {
        assertThrows(UnauthorizedException.class, () -> Sdk.admin().beta().environments().list());
    }
}
