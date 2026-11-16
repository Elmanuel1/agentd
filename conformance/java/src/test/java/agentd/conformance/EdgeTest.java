package agentd.conformance;

import static agentd.conformance.Sdk.client;
import static agentd.conformance.Sdk.name;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.anthropic.errors.AnthropicServiceException;
import com.anthropic.errors.UnauthorizedException;
import com.anthropic.models.beta.memorystores.MemoryStoreCreateParams;
import com.anthropic.models.beta.memorystores.memories.MemoryCreateParams;
import org.junit.jupiter.api.Test;

class EdgeTest {
    @Test
    void theSdkKeyIsAccepted() {
        assertTrue(client().beta().environments().list().data() != null);
    }

    @Test
    void anUnknownKeyIsUnauthorizedWithARequestId() {
        var e = assertThrows(UnauthorizedException.class, () -> Sdk.client("not-a-key").beta().environments().list());
        assertTrue(e.headers().values("request-id").stream().anyMatch(id -> id.startsWith("req_")));
    }

    @Test
    void aConflictIsAConflict() {
        var c = client();
        var store = c.beta().memoryStores().create(MemoryStoreCreateParams.builder().name(name("java-conflict")).build());
        var memory = MemoryCreateParams.builder().memoryStoreId(store.id()).path("/a.md").content("one").build();
        c.beta().memoryStores().memories().create(memory);
        var again = MemoryCreateParams.builder().memoryStoreId(store.id()).path("/a.md").content("two").build();
        var e = assertThrows(AnthropicServiceException.class, () -> c.beta().memoryStores().memories().create(again));
        assertEquals(409, e.statusCode());
    }
}
