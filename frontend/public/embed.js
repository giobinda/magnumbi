/*
 * Magnum BI embed: <script src="https://<magnum-bi-host>/embed.js"></script>
 *   <magnum-dashboard dashboard="sales_overview" filters="category=Footwear,Outerwear&period=90"></magnum-dashboard>
 * Attributes: dashboard (required), filters (query-string of field=values), hide-filters, height (fixed px),
 *   theme (theme id, e.g. "midnight"; overrides the dashboard's own theme).
 * The iframe auto-resizes to its content unless `height` is set.
 */
(function () {
  if (customElements.get('magnum-dashboard')) return
  var origin = new URL(document.currentScript.src).origin

  class MagnumDashboard extends HTMLElement {
    static get observedAttributes() { return ['dashboard', 'filters', 'hide-filters', 'height', 'theme'] }
    connectedCallback() {
      if (!this.iframe) {
        this.iframe = document.createElement('iframe')
        this.iframe.style.cssText = 'width:100%;border:0;display:block;min-height:200px'
        this.iframe.setAttribute('title', 'Magnum BI dashboard')
        this.appendChild(this.iframe)
        this.onMessage = (e) => {
          if (e.origin !== origin || e.source !== this.iframe.contentWindow) return
          if (e.data && e.data.type === 'magnum:resize' && !this.hasAttribute('height')) this.iframe.style.height = e.data.height + 'px'
        }
        window.addEventListener('message', this.onMessage)
      }
      this.render()
    }
    disconnectedCallback() { window.removeEventListener('message', this.onMessage) }
    attributeChangedCallback() { if (this.iframe) this.render() }
    render() {
      var id = this.getAttribute('dashboard')
      if (!id) return
      var qs = new URLSearchParams(this.getAttribute('filters') || '')
      if (this.hasAttribute('hide-filters')) qs.set('filters', '0')
      if (this.getAttribute('theme')) qs.set('theme', this.getAttribute('theme'))
      var src = origin + '/embed/' + encodeURIComponent(id) + (qs.toString() ? '?' + qs : '')
      if (this.iframe.src !== src) this.iframe.src = src
      if (this.hasAttribute('height')) this.iframe.style.height = this.getAttribute('height') + 'px'
    }
  }
  customElements.define('magnum-dashboard', MagnumDashboard)
})()
