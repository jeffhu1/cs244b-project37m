import numpy as np
import networkx as nx
import heapq

GRAPH_FNAME = "graph.adjlist"

DEBUG = False

G = nx.read_adjlist(GRAPH_FNAME, create_using=nx.DiGraph())

print(G)
print(G.nodes())


# Compute time for node
class NodeExecutionTimePolicy(object):
    def constant_estimate(node):
        # For now, we use a constant time scaled to 125ms for 195 transactions
        return 125.0 / 195

    def gas_estimate(node):
        # TODO
        return 1.00

    def random_estimate(node):
        # random between 0.50 and 2
        return np.random.uniform(0.50, 2)


class NodeSelectionPolicies(object):
    def greedy_max_depth_width(view: nx.Graph, ignore_nodes=None):
        # Compute all components
        components = nx.weakly_connected_components(view)
        options = []
        heapq.heapify(options)
        for component in components:
            subgraph = view.subgraph(component)

            # Find all candidate nodes (i.e. in-degree zero)
            candidates = [
                node for node in subgraph.nodes() if subgraph.in_degree(node) == 0
            ]
            for candidate in candidates:
                if ignore_nodes is not None and candidate in ignore_nodes:
                    continue

                subtree = nx.dfs_tree(subgraph, source=candidate)
                depth = nx.dag_longest_path_length(subtree)
                width = len(max(nx.topological_generations(subtree), key=len))
                # options.append((depth, width, candidate))
                heapq.heappush(options, (-depth, -width, candidate))

        if not options:
            return None
        else:
            return heapq.heappop(options)[-1]

    def random_node(view: nx.Graph, ignore_nodes=None):
        candidates = [
            node
            for node in view.nodes()
            if ignore_nodes is None or node not in ignore_nodes
        ]
        if candidates:
            return np.random.choice(candidates)
        else:
            return None

    def next_txn_id_node(view: nx.Graph, ignore_nodes=None):
        candidates = [
            node
            for node in view.nodes()
            if ignore_nodes is None or node not in ignore_nodes
        ]
        candidates = sorted(candidates)
        if candidates:
            return candidates[0]
        else:
            return None


class Simulator(object):
    def __init__(self, graph: nx.Graph, rollback_penalty: float = 0.50) -> None:
        self.graph = graph
        self.rollback_penalty = rollback_penalty

    def simulate(
        self,
        node_selection_policy=NodeSelectionPolicies.greedy_max_depth_width,
        node_time_policy=NodeExecutionTimePolicy.constant_estimate,
        parallelism: int = 1,
    ):
        current_view = self.graph

        work_items = {
            i: None for i in range(parallelism)
        }  # store mapping from thread ID --> None or current work item (2-tuple of (node, absolute time of completion))

        timestep = 0.00
        current_invalid_nodes = set()  # keeps track of nodes that incurred a rollback
        while True:
            if DEBUG:
                print(
                    f"{timestep:.2f}: {len(current_view.nodes())} nodes left, {len(current_view.edges())} edges left, {work_items} work items left"
                )

            # Remove any completed work items
            for i in work_items:
                if work_items[i] is not None and work_items[i][1] <= timestep:
                    # filter current view to remove this node
                    current_view = current_view.subgraph(
                        current_view.nodes() - {work_items[i][0]}
                    )

                    # reset currently invalid nodes if an item actually completed
                    if work_items[i][0] is not None:
                        current_invalid_nodes.clear()

                    work_items[i] = None

            # Schedule next work item(s)
            for i in work_items:
                if work_items[i] is None:
                    next_node = node_selection_policy(
                        current_view,
                        ignore_nodes=[
                            work_items[i][0]
                            for i in work_items
                            if work_items[i] is not None
                        ]
                        + list(current_invalid_nodes),
                    )
                    if next_node is not None:
                        is_doable = current_view.in_degree(next_node) == 0
                        if is_doable:
                            work_items[i] = (
                                next_node,
                                timestep + node_time_policy(next_node),
                            )
                        else:
                            work_items[i] = (
                                None,
                                timestep + self.rollback_penalty,
                            )
                            current_invalid_nodes.add(next_node)

            # If right now, no work is left, we're done
            if all(value is None for value in work_items.values()):
                break

            # Shortest timestep to completion of next work item(s)
            timestep += (
                min([value[1] for value in work_items.values() if value is not None])
                - timestep
            )  # min of all work items

        # print(f"Done in {timestep:.2f}s")
        return timestep


simulator = Simulator(G)

for node_time_policy in [NodeExecutionTimePolicy.constant_estimate, NodeExecutionTimePolicy.random_estimate]:
    for node_selection_policy in [
        NodeSelectionPolicies.greedy_max_depth_width,
        NodeSelectionPolicies.random_node,
        NodeSelectionPolicies.next_txn_id_node,
    ]:
        
        timings = {}
        print("node_time_policy, node_selection_policy")
        print(node_time_policy.__name__, node_selection_policy.__name__)

        # for parallelism in [1, 2, 3, 4, 5, 6, 7, 8]:
        for parallelism in [1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 14, 16, 20, 24, 32]:
            parallelism = int(parallelism)
            timings[parallelism] = simulator.simulate(
                parallelism=parallelism,
                node_time_policy=node_time_policy,
                node_selection_policy=node_selection_policy,
            )

        print(timings)

        # Print results to file for plotting
        with open(f"simulations/{node_time_policy.__name__}-{node_selection_policy.__name__}.csv", "w") as f:
            f.write("parallelism,time\n")
            for parallelism in timings:
                f.write(f"{parallelism},{timings[parallelism]}\n")

exit(0)

# plot simulated time vs parallelism
import matplotlib.pyplot as plt
import seaborn as sns

# Labels and title using seaborn

sns.set_theme()
plt.xlabel("Parallelism")
plt.ylabel("Time (ms)")
plt.title("Simulated time vs. parallelism")

# plot horizontal line at 125ms
plt.axhline(y=125, color="y", linestyle="-")
plt.yscale("log")
plt.plot(timings.keys(), timings.values())
plt.show()
