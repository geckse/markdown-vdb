<script lang="ts">
  import { onDestroy } from 'svelte';
  import { forceSimulation, forceLink, forceManyBody, forceCenter, forceCollide } from 'd3-force';
  import { drag as d3Drag } from 'd3-drag';
  import { select } from 'd3-selection';
  import type { Simulation, SimulationNodeDatum, SimulationLinkDatum } from 'd3-force';

  export interface GraphNode extends SimulationNodeDatum {
    id: string;
    label?: string;
    isCenter?: boolean;
    cluster?: number;
  }

  export interface GraphEdge extends SimulationLinkDatum<GraphNode> {
    source: string | GraphNode;
    target: string | GraphNode;
    direction?: 'outbound' | 'inbound' | 'bidirectional';
    strength?: number;
  }

  interface ForceGraphProps {
    nodes: GraphNode[];
    edges: GraphEdge[];
    width?: number;
    height?: number;
    interactive?: boolean;
    showLabels?: boolean;
    glowEdges?: boolean;
  }

  let {
    nodes,
    edges,
    width = 600,
    height = 400,
    interactive = true,
    showLabels = true,
    glowEdges = true,
  }: ForceGraphProps = $props();

  const CLUSTER_COLORS: string[] = [
    '#A78BFA', '#67E8F9', '#FCA5A1', '#86EFAC',
    '#FDE68A', '#F9A8D4', '#FDBA74', '#93C5FD',
  ];

  let simNodes: GraphNode[] = $state([]);
  let simEdges: GraphEdge[] = $state([]);
  let hoveredNodeId: string | null = $state(null);
  let simulation: Simulation<GraphNode, GraphEdge> | null = null;
  let svgEl: SVGSVGElement | undefined = $state(undefined);
  let reducedMotion = $state(false);

  // Check prefers-reduced-motion
  $effect(() => {
    if (typeof window !== 'undefined') {
      const mq = window.matchMedia('(prefers-reduced-motion: reduce)');
      reducedMotion = mq.matches;
    }
  });

  let connectedEdges: Set<number> = $derived.by(() => {
    if (!hoveredNodeId) return new Set();
    const set = new Set<number>();
    simEdges.forEach((e, i) => {
      const src = typeof e.source === 'string' ? e.source : (e.source as GraphNode).id;
      const tgt = typeof e.target === 'string' ? e.target : (e.target as GraphNode).id;
      if (src === hoveredNodeId || tgt === hoveredNodeId) set.add(i);
    });
    return set;
  });

  let connectedNodeIds: Set<string> = $derived.by(() => {
    if (!hoveredNodeId) return new Set();
    const set = new Set<string>([hoveredNodeId]);
    simEdges.forEach((e) => {
      const src = typeof e.source === 'string' ? e.source : (e.source as GraphNode).id;
      const tgt = typeof e.target === 'string' ? e.target : (e.target as GraphNode).id;
      if (src === hoveredNodeId) set.add(tgt);
      if (tgt === hoveredNodeId) set.add(src);
    });
    return set;
  });

  function getNodeColor(node: GraphNode): string {
    if (node.isCenter) return '#22d3ee';
    if (node.cluster != null) return CLUSTER_COLORS[((node.cluster % 8) + 8) % 8];
    return '#9ca3af';
  }

  function getEdgeColor(edge: GraphEdge): string {
    switch (edge.direction) {
      case 'outbound': return '#22d3ee';
      case 'inbound': return '#ef4444';
      case 'bidirectional': return '#22c55e';
      default: return '#6b7280';
    }
  }

  function getEdgeWidth(edge: GraphEdge): number {
    const s = edge.strength ?? 0.5;
    return 0.5 + s * 2;
  }

  function getEdgeDash(edge: GraphEdge): string | undefined {
    const s = edge.strength ?? 0.5;
    return s < 0.3 ? '3,3' : undefined;
  }

  function getEdgeOpacity(index: number): number {
    if (!hoveredNodeId) return 1;
    return connectedEdges.has(index) ? 1 : 0.15;
  }

  function getNodeOpacity(node: GraphNode): number {
    if (!hoveredNodeId) return 1;
    return connectedNodeIds.has(node.id) ? 1 : 0.25;
  }

  function runSimulation() {
    if (simulation) simulation.stop();
    if (nodes.length === 0) {
      simNodes = [];
      simEdges = [];
      return;
    }

    const clonedNodes = nodes.map(n => ({ ...n }));
    const clonedEdges = edges.map(e => ({ ...e }));

    simulation = forceSimulation<GraphNode>(clonedNodes)
      .force(
        'link',
        forceLink<GraphNode, GraphEdge>(clonedEdges)
          .id((d) => d.id)
          .distance(80)
      )
      .force('charge', forceManyBody<GraphNode>().strength(-120))
      .force('center', forceCenter(width / 2, height / 2))
      .force('collide', forceCollide<GraphNode>().radius((d) => ((d as GraphNode).isCenter ? 10 : 6) + 4))
      .alphaDecay(0.05)
      .velocityDecay(0.4);

    // Warmup ticks
    simulation.tick(100);

    if (reducedMotion) {
      simulation.stop();
      simEdges = clonedEdges;
      simNodes = [...clonedNodes];
      return;
    }

    simulation.stop();
    simEdges = clonedEdges;
    simNodes = [...clonedNodes];

    if (interactive) {
      simulation.alpha(0.3).restart();
      simulation.on('tick', () => {
        simNodes = [...clonedNodes];
        simEdges = [...clonedEdges];
      });
    }
  }

  // Setup drag behavior
  $effect(() => {
    if (!svgEl || !interactive || !simulation || reducedMotion) return;

    const svg = select(svgEl);
    const nodeEls = svg.selectAll<SVGCircleElement, GraphNode>('.graph-node');

    const dragBehavior = d3Drag<SVGCircleElement, GraphNode>()
      .on('start', (event, d) => {
        if (!event.active && simulation) simulation.alphaTarget(0.3).restart();
        d.fx = d.x;
        d.fy = d.y;
      })
      .on('drag', (event, d) => {
        d.fx = event.x;
        d.fy = event.y;
      })
      .on('end', (event, d) => {
        if (!event.active && simulation) simulation.alphaTarget(0);
        d.fx = null;
        d.fy = null;
      });

    nodeEls.data(simNodes, (d: GraphNode) => d.id).call(dragBehavior);
  });

  $effect(() => {
    nodes;
    edges;
    width;
    height;
    runSimulation();
  });

  onDestroy(() => {
    if (simulation) {
      simulation.stop();
      simulation = null;
    }
  });

  function edgeX1(e: GraphEdge): number { return typeof e.source === 'string' ? 0 : (e.source as GraphNode).x ?? 0; }
  function edgeY1(e: GraphEdge): number { return typeof e.source === 'string' ? 0 : (e.source as GraphNode).y ?? 0; }
  function edgeX2(e: GraphEdge): number { return typeof e.target === 'string' ? 0 : (e.target as GraphNode).x ?? 0; }
  function edgeY2(e: GraphEdge): number { return typeof e.target === 'string' ? 0 : (e.target as GraphNode).y ?? 0; }
</script>

<svg
  bind:this={svgEl}
  {width}
  {height}
  viewBox="0 0 {width} {height}"
  class="force-graph"
  role="img"
  aria-label="Force-directed graph visualization"
>
  <defs>
    <filter id="neon-glow" x="-50%" y="-50%" width="200%" height="200%">
      <feGaussianBlur in="SourceGraphic" stdDeviation="3" result="blur" />
      <feComposite in="SourceGraphic" in2="blur" operator="over" />
    </filter>
  </defs>

  {#each simEdges as edge, i}
    <line
      x1={edgeX1(edge)}
      y1={edgeY1(edge)}
      x2={edgeX2(edge)}
      y2={edgeY2(edge)}
      stroke={getEdgeColor(edge)}
      stroke-width={getEdgeWidth(edge)}
      stroke-dasharray={getEdgeDash(edge)}
      opacity={getEdgeOpacity(i)}
      filter={glowEdges ? 'url(#neon-glow)' : undefined}
    />
  {/each}

  {#each simNodes as node}
    <g
      class="graph-node"
      transform="translate({node.x ?? 0},{node.y ?? 0})"
      opacity={getNodeOpacity(node)}
      onpointerenter={() => hoveredNodeId = node.id}
      onpointerleave={() => hoveredNodeId = null}
    >
      <circle
        r={node.isCenter ? 10 : 6}
        fill={getNodeColor(node)}
        filter={node.isCenter ? 'url(#neon-glow)' : undefined}
        stroke={node.isCenter ? '#22d3ee' : 'none'}
        stroke-width={node.isCenter ? 1.5 : 0}
      />
      {#if showLabels && (hoveredNodeId === node.id || node.isCenter)}
        <rect
          x={14}
          y={-8}
          width={(node.label ?? node.id).length * 6.5 + 8}
          height={16}
          rx={3}
          fill="rgba(15, 15, 30, 0.85)"
        />
        <text
          x={18}
          y={4}
          font-size="11"
          font-family="'Space Grotesk', sans-serif"
          fill="#e2e8f0"
        >
          {node.label ?? node.id}
        </text>
      {/if}
    </g>
  {/each}
</svg>

<style>
  .force-graph {
    overflow: visible;
  }

  .graph-node {
    cursor: grab;
  }

  .graph-node:active {
    cursor: grabbing;
  }
</style>
