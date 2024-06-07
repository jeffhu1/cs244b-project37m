import os
import sys
import subprocess
import numpy as np
import pandas as pd
import matplotlib.pyplot as plt
import seaborn as sns

RESULT_FNAME = os.path.join(os.path.dirname(__file__), "results.csv")

# only run if not invoked with "--resume"
if len(sys.argv) == 1 or sys.argv[1] not in ("--resume", "-r"):
    timings = []
    for parallelism in [1, 2, 3, 4, 5, 6, 7, 8]:
        for ssd_delay in [0, 5, 10, 20, 50]:
            if parallelism == 1:
                prefix = "Sequential transactions per second"
                cmd = f'cargo run --example compare --features="example_utils" --release -- -d {ssd_delay} -t {parallelism} -s'
            else:
                prefix = "Transactions per second"
                cmd = f'cargo run --example compare --features="example_utils" --release -- -d {ssd_delay} -t {parallelism}'

            print(f"Running with parallelism {parallelism} and ssd_delay {ssd_delay}")
            output = subprocess.run(cmd, shell=True, capture_output=True, text=True)
            lines = output.stdout.split("\n")

            for line in lines:
                if line.startswith(prefix):
                    tps = float(line.split(":")[-1])
                    break

            print(f"TPS: {tps}")
            timings.append(
                {"parallelism": parallelism, "ssd_delay": ssd_delay, "tps": tps}
            )

    df = pd.DataFrame(timings)
    df.to_csv(RESULT_FNAME, index=False)
else:
    df = pd.read_csv(RESULT_FNAME)

# plt.ylim(0, None)
sns.set_style("whitegrid")

# Increase font size
plt.rcParams.update({"font.size": 14})

# Increase figure size
plt.gcf().set_size_inches(8, 6)

# Add labels and title
plt.xlabel("Parallelism (number of threads)")
plt.ylabel("Transactions per Second (TPS)")
plt.title("TPS vs. Parallelism")

# plot tps vs parallelism by ssd delay with seaborn
# log y scale
g_results = sns.lineplot(
    x="parallelism", y="tps", hue="ssd_delay", data=df, markers=True
)
g_results.set(yscale="log")

# add more y ticks
yticks = [2 * 10**3, 5 * 10**3, 10**4, 2 * 10**4]
g_results.set_yticks(yticks)
g_results.set_yticklabels(yticks)

# Add legend
plt.legend(title="SSD Delay (μs)")

plt.savefig("sweep.pdf")

plt.show()
