// What the browser attack plants: each sets window.__xss if a browser ever runs it.
export const PAYLOADS = [
  `<img src=x onerror="window.__xss='img'">`,
  `<script>window.__xss='script'</script>`,
  `"><svg onload="window.__xss='svg'">`,
  `javascript:window.__xss='href'`,
  `{{constructor.constructor("window.__xss='template'")()}}`,
  `<a href="javascript:window.__xss='a'">x</a>`,
]
