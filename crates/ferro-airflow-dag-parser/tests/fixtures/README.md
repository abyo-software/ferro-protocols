# Airflow DAG conformance fixtures

Real Airflow example DAGs used by `tests/conformance.rs`.

## Sources

- `example_bash_operator.py` — vendored from
  <https://github.com/apache/airflow/blob/main/airflow/example_dags/example_bash_operator.py>
- `tutorial.py` — vendored from
  <https://github.com/apache/airflow/blob/main/airflow/example_dags/tutorial.py>

License compliance: both files are Apache-2.0 (Apache Airflow). The
upstream license header is preserved verbatim at the top of each file.
The Apache License §4 explicitly permits redistribution of source files
provided the license notice is included, which we satisfy.

## Coverage rationale

- `example_bash_operator.py` is the canonical fan-out / fan-in shape:
  three parallel `BashOperator` tasks → one `BashOperator` →
  two parallel ops → one `EmptyOperator`. It exercises the
  `[a, b, c] >> d` list-shift edge form which is unique to Airflow's
  static DAG syntax.
- `tutorial.py` is Airflow's first-tutorial example. It covers
  `default_args`, multi-line dedented templates, `dag.doc_md`
  assignment from `__doc__`, and the `t1 >> [t2, t3]` shift edge form
  with retries/email_on_failure metadata.

A static parser that recovers `dag_id`, the full `task_id` set, and the
edge graph from these two real DAGs is materially compatible with the
Airflow scheduler's parse-time DAG discovery.
