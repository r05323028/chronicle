package example.webmvc;

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RestController;

/**
 * Deterministic endpoints: Chronicle replay compares status, body digest, and
 * headers against the recording, so no timestamps, UUIDs, or random values.
 */
@RestController
public class HelloController {

    @GetMapping("/hello")
    public String hello() {
        return "Hello, Chronicle!";
    }

    @PostMapping("/echo")
    public String echo(@RequestBody String body) {
        return body;
    }
}
