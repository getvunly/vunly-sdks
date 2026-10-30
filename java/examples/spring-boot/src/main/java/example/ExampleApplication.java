package example;

import com.vunly.Vunly;
import java.time.Duration;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.context.annotation.Bean;

/**
 * A small web service that meters its own work with Vunly. The client is one bean, built from properties; a Spring Boot
 * starter would add nothing to these ten lines.
 *
 * <pre>
 * VUNLY_API_URL=http://127.0.0.1:8787 VUNLY_API_KEY=vk_test_... java -jar target/vunly-spring-boot-example-0.1.0.jar
 * </pre>
 */
@SpringBootApplication
public class ExampleApplication {

  /**
   * The key and the address come from the environment (relaxed binding maps VUNLY_API_KEY to vunly.api-key). Nothing in
   * this application prints them: there is no actuator, no /env, and the client's toString hides the key.
   */
  @Bean
  Vunly vunly(
      @Value("${vunly.api-key}") String apiKey,
      @Value("${vunly.api-url}") String apiUrl,
      @Value("${vunly.timeout:10s}") Duration timeout) {
    return Vunly.builder().apiKey(apiKey).baseUrl(apiUrl).timeout(timeout).build();
  }

  public static void main(String[] args) {
    SpringApplication.run(ExampleApplication.class, args);
  }
}
