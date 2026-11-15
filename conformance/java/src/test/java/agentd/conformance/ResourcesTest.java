package agentd.conformance;

import static agentd.conformance.Sdk.client;
import static agentd.conformance.Sdk.json;
import static agentd.conformance.Sdk.map;
import static agentd.conformance.Sdk.name;
import static agentd.conformance.Sdk.tree;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.anthropic.models.beta.memorystores.MemoryStoreCreateParams;
import com.anthropic.models.beta.memorystores.MemoryStoreListParams;
import com.anthropic.models.beta.memorystores.memories.MemoryCreateParams;
import com.anthropic.models.beta.memorystores.memories.MemoryDeleteParams;
import com.anthropic.models.beta.vaults.VaultCreateParams;
import com.anthropic.models.beta.vaults.credentials.CredentialArchiveParams;
import com.anthropic.models.beta.vaults.credentials.CredentialCreateParams;
import com.anthropic.models.beta.vaults.credentials.CredentialRetrieveParams;
import java.util.UUID;
import java.util.stream.Collectors;
import org.junit.jupiter.api.Test;

class ResourcesTest {
    @Test
    void aMemoryStoreLeavesTheDefaultListOnceArchived() {
        var c = client();
        var s = c.beta().memoryStores().create(MemoryStoreCreateParams.builder().name(name("java-store")).build());
        c.beta().memoryStores().archive(s.id());
        var params = MemoryStoreListParams.builder().limit(100).build();
        var live = c.beta().memoryStores().list(params).autoPager().stream().map(x -> x.id()).collect(Collectors.toSet());
        assertFalse(live.contains(s.id()));
        var all = c.beta().memoryStores().list(MemoryStoreListParams.builder().limit(100).includeArchived(true).build());
        assertTrue(all.data().stream().anyMatch(x -> x.id().equals(s.id())));
    }

    @Test
    void memoriesAreCreatedAndDeleted() {
        var c = client();
        var store = c.beta().memoryStores().create(MemoryStoreCreateParams.builder().name(name("java-mem")).build());
        var memory = c.beta().memoryStores().memories().create(
                MemoryCreateParams.builder().memoryStoreId(store.id()).path("/notes/a.md").content("hello").build());
        assertEquals("/notes/a.md", memory.path());
        var deleted = tree(c.beta().memoryStores().memories().delete(memory.id(),
                MemoryDeleteParams.builder().memoryStoreId(store.id()).build()));
        assertEquals("memory_deleted", deleted.get("type").asText());
        var gone = tree(c.beta().memoryStores().delete(store.id()));
        assertEquals("memory_store_deleted", gone.get("type").asText());
    }

    @Test
    void aCredentialSecretIsStoredAndNeverReturned() {
        var c = client();
        var vault = c.beta().vaults().create(VaultCreateParams.builder().displayName(name("java-vault")).build());
        var token = "java-token-" + UUID.randomUUID();
        var auth = map("type", "static_bearer", "mcp_server_url", "https://mcp.example.com/mcp", "token", token);
        var credential = c.beta().vaults().credentials().create(vault.id(), CredentialCreateParams.builder()
                .displayName(name("java-cred"))
                .auth(Sdk.<CredentialCreateParams.Auth>field(auth)).build());
        var fetched = tree(c.beta().vaults().credentials().retrieve(credential.id(),
                CredentialRetrieveParams.builder().vaultId(vault.id()).build()));
        assertEquals("static_bearer", fetched.at("/auth/type").asText());
        assertFalse(fetched.toString().contains(token));
        var archived = tree(c.beta().vaults().credentials().archive(credential.id(),
                CredentialArchiveParams.builder().vaultId(vault.id()).build()));
        assertFalse(archived.get("archived_at").isNull());
        c.beta().vaults().delete(vault.id());
    }
}
