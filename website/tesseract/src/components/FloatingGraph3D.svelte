<script>
  import { onMount } from 'svelte';

  let container;

  onMount(() => {
    if (typeof window === 'undefined' || window.innerWidth < 768) return;

    let animationId;
    let renderer;
    let angle = 0;

    async function init() {
      const THREE = await import('three');
      const { Scene, WebGLRenderer, PerspectiveCamera, AmbientLight, DirectionalLight, MeshLambertMaterial } = THREE;
      const { default: ThreeForceGraph } = await import('three-forcegraph');

      if (!container) return;

      const width = container.clientWidth;
      const height = container.clientHeight;

      const scene = new Scene();
      const camera = new PerspectiveCamera(60, width / height, 1, 2000);
      camera.position.set(0, 50, 280);

      scene.add(new AmbientLight(0xffffff, 0.8));
      const dirLight = new DirectionalLight(0xffffff, 0.6);
      dirLight.position.set(100, 200, 100);
      scene.add(dirLight);

      renderer = new WebGLRenderer({ alpha: true, antialias: false });
      renderer.setClearColor(0x000000, 0);
      renderer.setPixelRatio(Math.min(devicePixelRatio, 2));
      renderer.setSize(width, height);
      container.appendChild(renderer.domElement);

      const CLUSTER_COLORS = [
        '#00E5FF', '#A78BFA', '#67E8F9', '#86EFAC', '#FCA5A1',
        '#FCD34D', '#60A5FA', '#34D399', '#FB7185'
      ];

      const nodes = [];
      const links = [];
      let nodeId = 0;

      for (let c = 0; c < 9; c++) {
        const hubId = nodeId++;
        nodes.push({ id: hubId, cluster: c, isHub: true });

        const spokeIds = [];
        for (let s = 0; s < 4; s++) {
          const sid = nodeId++;
          nodes.push({ id: sid, cluster: c, isHub: false });
          spokeIds.push(sid);
          links.push({ source: hubId, target: sid });
        }

        if (spokeIds.length >= 2) {
          const i = Math.floor(Math.random() * spokeIds.length);
          let j = (i + 1) % spokeIds.length;
          links.push({ source: spokeIds[i], target: spokeIds[j] });
        }
      }

      for (let i = 0; i < 10; i++) {
        const a = Math.floor(Math.random() * nodes.length);
        let b = Math.floor(Math.random() * nodes.length);
        if (a !== b && nodes[a].cluster !== nodes[b].cluster) {
          links.push({ source: nodes[a].id, target: nodes[b].id });
        }
      }

      const graph = new ThreeForceGraph()
        .graphData({ nodes, links })
        .nodeThreeObject((node) => {
          const color = CLUSTER_COLORS[node.cluster] || '#ffffff';
          const size = node.isHub ? 4 : 2.5;
          const geometry = new THREE.SphereGeometry(size, 16, 12);
          const material = new MeshLambertMaterial({ color });
          return new THREE.Mesh(geometry, material);
        })
        .linkColor(() => 'rgba(255,255,255,0.15)')
        .linkWidth(0.5)
        .d3Force('charge').strength(-80);

      graph.d3Force('link').distance(60);

      scene.add(graph);

      function animate() {
        animationId = requestAnimationFrame(animate);
        angle += 0.002;
        camera.position.x = Math.sin(angle) * 280;
        camera.position.z = Math.cos(angle) * 280;
        camera.lookAt(0, 0, 0);
        graph.tickFrame();
        renderer.render(scene, camera);
      }
      animate();

      function onResize() {
        if (!container) return;
        const w = container.clientWidth;
        const h = container.clientHeight;
        camera.aspect = w / h;
        camera.updateProjectionMatrix();
        renderer.setSize(w, h);
      }
      window.addEventListener('resize', onResize);

      return () => {
        window.removeEventListener('resize', onResize);
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
  class="absolute inset-0 w-full h-full pointer-events-none opacity-50 hidden md:block"
></div>
