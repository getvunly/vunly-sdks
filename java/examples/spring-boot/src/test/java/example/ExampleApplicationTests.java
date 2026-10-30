package example;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;

import com.vunly.Vunly;
import org.junit.jupiter.api.Test;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.boot.test.context.SpringBootTest;

@SpringBootTest(
    properties = {
      "vunly.api-url=http://127.0.0.1:1",
      "vunly.api-key=vk_test_synthetic_000000000000000000",
      "server.port=0"
    })
class ExampleApplicationTests {
  @Autowired Vunly vunly;

  @Test
  void theClientIsABeanBuiltFromProperties() {
    assertNotNull(vunly);
    assertFalse(vunly.toString().contains("vk_test_synthetic"), "the client printed its key");
  }
}
