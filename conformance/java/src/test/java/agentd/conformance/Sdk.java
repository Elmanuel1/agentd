package agentd.conformance;

import com.anthropic.client.AnthropicClient;
import com.anthropic.client.okhttp.AnthropicOkHttpClient;
import com.anthropic.core.JsonValue;
import com.anthropic.core.ObjectMappers;
import com.fasterxml.jackson.databind.JsonNode;
import java.util.Map;
import java.util.UUID;

/**
 * The official Java SDK against a running agentd, with only the base URL changed. AGENTD_BASE_URL
 * (default http://127.0.0.1:8080) names the server, AGENTD_TEST_KEY a workspace key it accepts and
 * AGENTD_TEST_ADMIN_KEY an admin key for the same organization.
 */
final class Sdk {
    static final String BASE_URL = System.getenv().getOrDefault("AGENTD_BASE_URL", "http://127.0.0.1:8080");

    private Sdk() {}

    static AnthropicClient client() {
        return client(System.getenv("AGENTD_TEST_KEY"));
    }

    static AnthropicClient admin() {
        return client(System.getenv("AGENTD_TEST_ADMIN_KEY"));
    }

    static AnthropicClient client(String key) {
        return AnthropicOkHttpClient.builder().apiKey(key).baseUrl(BASE_URL).build();
    }

    static String name(String prefix) {
        return prefix + "-" + UUID.randomUUID().toString().substring(0, 8);
    }

    /** A request body field, as JSON. */
    static JsonValue json(Object value) {
        return JsonValue.from(value);
    }

    /** A required field the typed builders take, given as JSON. */
    @SuppressWarnings("unchecked")
    static <T> com.anthropic.core.JsonField<T> field(Object value) {
        return (com.anthropic.core.JsonField<T>) (com.anthropic.core.JsonField<?>) JsonValue.from(value);
    }

    /** A response as the JSON the SDK read it from. */
    static JsonNode tree(Object response) {
        return ObjectMappers.jsonMapper().valueToTree(response);
    }

    static Map<String, Object> map(Object... pairs) {
        var m = new java.util.LinkedHashMap<String, Object>();
        for (int i = 0; i + 1 < pairs.length; i += 2) {
            m.put((String) pairs[i], pairs[i + 1]);
        }
        return m;
    }
}
