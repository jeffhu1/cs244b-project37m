# plot simulated time vs parallelism
import matplotlib.pyplot as plt
import seaborn as sns

from simulator import NodeExecutionTimePolicy, NodeSelectionPolicies, filename_for_simluation

node_selection_policy = NodeSelectionPolicies.next_txn_id_node
node_time_policy = NodeExecutionTimePolicy.random_estimate

# read timings from file
timings = {}
with open(f"simulations/{filename_for_simluation(node_selection_policy, node_time_policy)}.csv", "r") as f:
    # skip first line, contains header
    f.readline()
    for line in f.readlines():
        parallelism, time = line.split(",")
        timings[int(parallelism)] = float(time)

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
