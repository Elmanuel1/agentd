package agentd.conformance;

import static agentd.conformance.Sdk.client;
import static agentd.conformance.Sdk.json;
import static agentd.conformance.Sdk.map;
import static agentd.conformance.Sdk.name;
import static agentd.conformance.Sdk.tree;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.anthropic.models.beta.agents.AgentCreateParams;
import com.anthropic.models.beta.agents.AgentUpdateParams;
import com.anthropic.models.beta.agents.BetaManagedAgentsModel;
import com.anthropic.models.beta.environments.EnvironmentCreateParams;
import com.anthropic.models.beta.environments.EnvironmentListParams;
import java.util.List;
import java.util.stream.Collectors;
import org.junit.jupiter.api.Test;

class AgentsEnvironmentsTest {
    static AgentCreateParams.Builder agent(String prefix) {
        return AgentCreateParams.builder().name(name(prefix)).model(BetaManagedAgentsModel.of("claude-sonnet-4-6"));
    }

    @Test
    void anAgentServesEveryRequiredField() {
        var tools = List.of(map("type", "agent_toolset_20260401", "configs", List.of(map("name", "bash", "enabled", false))));
        var created = tree(client().beta().agents().create(agent("java-agent").putAdditionalBodyProperty("tools", json(tools)).build()));
        assertEquals("service_account", created.at("/execution_identity/type").asText());
        var config = created.at("/tools/0/configs/0");
        assertEquals(false, config.get("enabled").asBoolean());
        assertEquals("bash", config.get("name").asText());
        assertEquals("always_allow", config.at("/permission_policy/type").asText());
        assertEquals("bash", config.get("type").asText());
    }

    @Test
    void anUpdateNeedsNoVersionAndNullClears() {
        var c = client();
        var made = c.beta().agents().create(agent("java-upd").description("x").build());
        var updated = c.beta().agents().update(made.id(), AgentUpdateParams.builder().putAdditionalBodyProperty("description", json(null)).build());
        assertTrue(updated.description().isEmpty());
        assertEquals(made.version() + 1, updated.version());
    }

    @Test
    void archivedEnvironmentsAreListedOnlyOnRequest() {
        var c = client();
        var env = c.beta().environments().create(EnvironmentCreateParams.builder().name(name("java-env")).build());
        c.beta().environments().archive(env.id());
        var active = c.beta().environments().list().autoPager().stream().map(e -> e.id()).collect(Collectors.toSet());
        assertFalse(active.contains(env.id()));
        var params = EnvironmentListParams.builder().includeArchived(true).build();
        var all = c.beta().environments().list(params).autoPager().stream().map(e -> e.id()).collect(Collectors.toSet());
        assertTrue(all.contains(env.id()));
    }

    @Test
    void anEnvironmentDeleteReturnsTheDeletedBody() {
        var c = client();
        var env = c.beta().environments().create(EnvironmentCreateParams.builder().name(name("java-env"))
                .putAdditionalBodyProperty("config", json(map("type", "cloud", "networking", map("type", "unrestricted")))).build());
        var deleted = tree(c.beta().environments().delete(env.id()));
        assertEquals(env.id(), deleted.get("id").asText());
        assertEquals("environment_deleted", deleted.get("type").asText());
    }

    @Test
    void agentsAutoPaginateAcrossPages() {
        var c = client();
        var made = new java.util.HashSet<String>();
        for (int i = 0; i < 3; i++) {
            made.add(c.beta().agents().create(agent("java-page").build()).id());
        }
        var params = com.anthropic.models.beta.agents.AgentListParams.builder().limit(1).build();
        var seen = c.beta().agents().list(params).autoPager().stream().map(a -> a.id()).collect(Collectors.toSet());
        assertTrue(seen.containsAll(made));
    }
}
