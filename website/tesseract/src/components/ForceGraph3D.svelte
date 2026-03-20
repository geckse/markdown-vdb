<script>
  import { onMount } from 'svelte';

  let { nodes = [], edges = [], height = 320 } = $props();

  let container;

  const GROUP_COLORS = {
    research: '#A78BFA',
    writing: '#67E8F9',
    reference: '#FCA5A1',
    project: '#86EFAC'
  };

  const EDGE_COLORS = {
    outbound: '#00E5FF',
    inbound: '#FF6B6B',
    bidirectional: '#51CF66',
    semantic: '#A78BFA'
  };

  onMount(() => {
    if (typeof window === 'undefined' || window.innerWidth < 768) return;

    let animationId;
    let renderer;
    let resizeObserver;
    let angle = 0;

    async function init() {
      const THREE = await import('three');
      const { Scene, WebGLRenderer, PerspectiveCamera, AmbientLight, DirectionalLight, MeshLambertMaterial } = THREE;
      const { default: ThreeForceGraph } = await import('three-forcegraph');

      if (!container) return;

      const width = container.clientWidth;
      const h = height;

      const scene = new Scene();
      const camera = new PerspectiveCamera(60, width / h, 1, 2000);
      camera.position.set(0, 30, 150);

      scene.add(new AmbientLight(0xffffff, 0.8));
      const dirLight = new DirectionalLight(0xffffff, 0.6);
      dirLight.position.set(100, 200, 100);
      scene.add(dirLight);

      renderer = new WebGLRenderer({ alpha: true, antialias: false });
      renderer.setClearColor(0x000000, 0);
      renderer.setPixelRatio(Math.min(devicePixelRatio, 2));
      renderer.setSize(width, h);
      container.appendChild(renderer.domElement);

      const graphNodes = nodes.map((n) => ({
        id: n.id,
        label: n.label,
        group: n.group,
        isCenter: n.isCenter,
        val: n.isCenter ? 8 : 3
      }));

      const graphLinks = edges.map((e) => ({
        source: e.source,
        target: e.target,
        type: e.type,
        strength: e.strength
      }));

      const graph = new ThreeForceGraph()
        .graphData({ nodes: graphNodes, links: graphLinks })
        .nodeThreeObject((node) => {
          const color = GROUP_COLORS[node.group] || '#67E8F9';
          const size = node.isCenter ? 5 : 2.5;
          const geometry = new THREE.SphereGeometry(size, 16, 12);
          const material = new MeshLambertMaterial({ color });
          return new THREE.Mesh(geometry, material);
        })
        .linkColor((link) => EDGE_COLORS[link.type] || '#00E5FF')
        .linkWidth(0.5)
        .d3Force('charge').strength(-60);

      graph.d3Force('link').distance(30);

      scene.add(graph);

      function animate() {
        animationId = requestAnimationFrame(animate);
        angle += 0.003;
        camera.position.x = Math.sin(angle) * 150;
        camera.position.z = Math.cos(angle) * 150;
        camera.lookAt(0, 0, 0);
        graph.tickFrame();
        renderer.render(scene, camera);
      }
      animate();

      resizeObserver = new ResizeObserver(() => {
        if (!container) return;
        const w = container.clientWidth;
        camera.aspect = w / h;
        camera.updateProjectionMatrix();
        renderer.setSize(w, h);
      });
      resizeObserver.observe(container);

      return () => {
        if (resizeObserver) resizeObserver.disconnect();
        if (animationId) cancelAnimationFrame(animationId);
        if (renderer) {
          renderer.dispose();
          if (renderer.domElement && renderer.domElement.parentNode) {
            renderer.domElement.parentNode.removeChild(renderer.domElement);
          }
        }
      };
    }

    let cleanup;
    init().then((fn) => { cleanup = fn; });

    return () => {
      if (cleanup) cleanup();
    };
  });
</script>

<div
  bind:this={container}
  style="height: {height}px;"
  class="w-full"
></div>
