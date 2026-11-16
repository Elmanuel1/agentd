package agentd.conformance;

import static agentd.conformance.Sdk.client;
import static agentd.conformance.Sdk.json;
import static agentd.conformance.Sdk.map;
import static agentd.conformance.Sdk.name;
import static agentd.conformance.Sdk.tree;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.anthropic.models.beta.environments.EnvironmentCreateParams;
import com.anthropic.models.beta.sessions.SessionCreateParams;
import com.anthropic.models.beta.sessions.events.EventListParams;
import com.anthropic.models.beta.sessions.events.EventSendParams;
import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.Test;

class SessionsTest {
    @Test
    void aMessageRunsTheSessionAndADeleteRemovesIt() throws InterruptedException {
        var c = client();
        var agent = c.beta().agents().create(AgentsEnvironmentsTest.agent("java-sess").build());
        var env = c.beta().environments().create(EnvironmentCreateParams.builder().name(name("java-sess-env")).build());
        var session = c.beta().sessions().create(SessionCreateParams.builder().agent(agent.id()).environmentId(env.id()).build());
        var message = map("type", "user.message", "content", List.of(map("type", "text", "text", "hi")));
        c.beta().sessions().events().send(session.id(),
                EventSendParams.builder().events(Sdk.field(List.of(message))).build());
        List<String> types = new ArrayList<>();
        for (int i = 0; i < 100 && !types.contains("session.status_idle"); i++) {
            Thread.sleep(100);
            types.clear();
            var page = c.beta().sessions().events().list(session.id(), EventListParams.builder().limit(1000).build());
            page.data().forEach(e -> types.add(tree(e).get("type").asText()));
        }
        assertTrue(types.contains("session.status_running"), types.toString());
        assertTrue(types.contains("session.status_idle"), types.toString());
        var deleted = tree(c.beta().sessions().delete(session.id()));
        assertEquals("session_deleted", deleted.get("type").asText());
    }
}
